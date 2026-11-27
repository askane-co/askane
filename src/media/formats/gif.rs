use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::formats::RawDecodedImage;
use crate::media::image::{PixelFormat, SourceFormat};
use crate::media::{
    check_deadline, err_corrupt, err_limit, err_unsupported, temp_budget_guard, validate_dims,
    MAX_DIM, MAX_GIF_FRAMES, MAX_MEDIA_BYTES, MAX_SOURCE_BYTES,
};

fn read_sub_blocks(data: &[u8], mut pos: usize) -> Result<(Vec<u8>, usize), String> {
    let mut out = Vec::new();
    loop {
        if pos >= data.len() {
            return Err(err_corrupt("Truncated GIF sub-block chain"));
        }
        let len = data[pos] as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        if pos.checked_add(len).is_none_or(|e| e > data.len()) {
            return Err(err_corrupt("GIF sub-block overruns EOF"));
        }
        if out.len().saturating_add(len) > MAX_SOURCE_BYTES {
            return Err(err_limit("GIF sub-block stream exceeds 64 MiB limit"));
        }
        out.extend_from_slice(&data[pos..pos + len]);
        pos += len;
    }
    Ok((out, pos))
}

fn lzw_decode(
    min_code_size: u8,
    compressed: &[u8],
    expected_len: usize,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Vec<u8>, String> {
    if !(2..=8).contains(&min_code_size) {
        return Err(err_corrupt(format!(
            "Invalid GIF LZW min_code_size {}",
            min_code_size
        )));
    }
    let clear_code: u16 = 1u16 << min_code_size;
    let eoi_code: u16 = clear_code + 1;
    let mut code_size = (min_code_size + 1) as u32;
    let mut next_code = eoi_code + 1;

    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    let mut stack = [0u8; 4096];
    for i in 0..clear_code as usize {
        suffix[i] = i as u8;
    }

    let mut out = Vec::with_capacity(expected_len.min(4 * 1024 * 1024));
    let mut bit_pos: usize = 0;
    let total_bits = compressed.len().saturating_mul(8);

    let read_code = |bit_pos: &mut usize, width: u32| -> Option<u16> {
        if *bit_pos + (width as usize) > total_bits {
            return None;
        }
        let mut val = 0u32;
        for b in 0..width {
            let bp = *bit_pos + b as usize;
            let bit = (compressed[bp >> 3] >> (bp & 7)) & 1;
            val |= (bit as u32) << b;
        }
        *bit_pos += width as usize;
        Some(val as u16)
    };

    let mut prev_code: Option<u16> = None;
    let mut first_char: u8 = 0;
    let mut steps = 0usize;

    while let Some(code) = read_code(&mut bit_pos, code_size) {
        steps += 1;
        if (steps & 4095) == 0 {
            check_deadline(deadline_check)?;
        }
        if code == clear_code {
            code_size = (min_code_size + 1) as u32;
            next_code = eoi_code + 1;
            prev_code = None;
            continue;
        }
        if code == eoi_code {
            break;
        }

        let cur = code as usize;
        let mut stack_len = 0usize;
        let mut trace = cur;

        if trace < next_code as usize {
            while trace >= clear_code as usize {
                if stack_len >= 4096 || trace >= 4096 {
                    return Err(err_corrupt("GIF LZW dictionary cycle/overflow"));
                }
                stack[stack_len] = suffix[trace];
                stack_len += 1;
                trace = prefix[trace] as usize;
            }
            if stack_len >= 4096 {
                return Err(err_corrupt("GIF LZW stack overflow"));
            }
            first_char = suffix[trace];
            stack[stack_len] = first_char;
            stack_len += 1;
        } else if trace == next_code as usize {
            let Some(prev) = prev_code else {
                return Err(err_corrupt("Invalid first GIF LZW KwKwK code"));
            };
            stack[stack_len] = first_char;
            stack_len += 1;
            let mut p_trace = prev as usize;
            while p_trace >= clear_code as usize {
                if stack_len >= 4096 || p_trace >= 4096 {
                    return Err(err_corrupt("GIF LZW dictionary cycle/overflow"));
                }
                stack[stack_len] = suffix[p_trace];
                stack_len += 1;
                p_trace = prefix[p_trace] as usize;
            }
            if stack_len >= 4096 {
                return Err(err_corrupt("GIF LZW stack overflow"));
            }
            first_char = suffix[p_trace];
            stack[stack_len] = first_char;
            stack_len += 1;
        } else {
            return Err(err_corrupt("GIF LZW code exceeds table size"));
        }

        for k in (0..stack_len).rev() {
            if out.len() < expected_len {
                out.push(stack[k]);
            }
        }

        if let Some(prev) = prev_code {
            if (next_code as usize) < 4096 {
                prefix[next_code as usize] = prev;
                suffix[next_code as usize] = first_char;
                next_code += 1;
                if (next_code as usize) == (1usize << code_size) && code_size < 12 {
                    code_size += 1;
                }
            }
        }
        prev_code = Some(code);
        if out.len() >= expected_len {
            break;
        }
    }

    if out.len() < expected_len {
        out.resize(expected_len, 0);
    }
    Ok(out)
}

pub fn decode_gif(
    data: &[u8],
    max_frames_opt: usize,
    budget: Option<&Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<RawDecodedImage, String> {
    if data.len() < 13 || (&data[0..6] != b"GIF87a" && &data[0..6] != b"GIF89a") {
        return Err(err_corrupt("Invalid GIF signature"));
    }
    let width = u16::from_le_bytes([data[6], data[7]]) as u32;
    let height = u16::from_le_bytes([data[8], data[9]]) as u32;
    let packed = data[10];
    let _bg_idx = data[11];
    let pixels = validate_dims(width, height, MAX_DIM)?;
    let frame_bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| err_limit("GIF frame byte size overflow"))?;
    if frame_bytes > MAX_MEDIA_BYTES {
        return Err(err_limit("GIF single frame exceeds 64 MiB limit"));
    }

    let mut pos = 13usize;
    let gct = if (packed & 0x80) != 0 {
        let count = 1usize << ((packed & 0x07) as usize + 1);
        let bytes = count * 3;
        if pos + bytes > data.len() {
            return Err(err_corrupt("Truncated GIF global color table"));
        }
        let mut pal = Vec::with_capacity(count);
        for chunk in data[pos..pos + bytes].chunks_exact(3) {
            pal.push([chunk[0], chunk[1], chunk[2]]);
        }
        pos += bytes;
        Some(pal)
    } else {
        None
    };

    let frame_cap = if max_frames_opt == 0 {
        MAX_GIF_FRAMES
    } else {
        max_frames_opt.min(MAX_GIF_FRAMES)
    };

    let w = width as usize;
    let h = height as usize;
    let mut canvas = vec![0u8; frame_bytes];
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut delays_ms: Vec<u16> = Vec::new();
    let mut loop_count: u16 = 0;

    let mut gce_disposal: u8 = 0;
    let mut gce_trans_idx: Option<u8> = None;
    let mut gce_delay_ms: u16 = 100;
    let mut total_decoded_bytes: usize = 0;
    let _canvas_guard = temp_budget_guard(budget, frame_bytes)?;

    while pos < data.len() {
        check_deadline(deadline_check)?;
        let introducer = data[pos];
        pos += 1;
        match introducer {
            0x3B => break, // Trailer
            0x21 => {
                if pos >= data.len() {
                    return Err(err_corrupt("Truncated GIF extension"));
                }
                let label = data[pos];
                pos += 1;
                if label == 0xF9 {
                    let (blk, next_pos) = read_sub_blocks(data, pos)?;
                    pos = next_pos;
                    if blk.len() >= 4 {
                        let flags = blk[0];
                        gce_disposal = (flags >> 2) & 0x07;
                        let has_trans = (flags & 0x01) != 0;
                        let delay_cs = u16::from_le_bytes([blk[1], blk[2]]);
                        gce_delay_ms = delay_cs.saturating_mul(10);
                        gce_trans_idx = if has_trans { Some(blk[3]) } else { None };
                    }
                } else if label == 0xFF {
                    if pos >= data.len() {
                        return Err(err_corrupt("Truncated GIF app extension"));
                    }
                    let hdr_len = data[pos] as usize;
                    pos += 1;
                    if pos + hdr_len > data.len() {
                        return Err(err_corrupt("Truncated GIF app header"));
                    }
                    let app_id = &data[pos..pos + hdr_len];
                    pos += hdr_len;
                    let (sub, next_pos) = read_sub_blocks(data, pos)?;
                    pos = next_pos;
                    if (app_id == b"NETSCAPE2.0" || app_id == b"ANIMEXTS1.0")
                        && sub.len() >= 3
                        && sub[0] == 1
                    {
                        loop_count = u16::from_le_bytes([sub[1], sub[2]]);
                    }
                } else {
                    let (_, next_pos) = read_sub_blocks(data, pos)?;
                    pos = next_pos;
                }
            }
            0x2C => {
                if frames.len() >= frame_cap {
                    return Err(err_limit(format!(
                        "GIF frame count exceeds {} limit",
                        frame_cap
                    )));
                }
                total_decoded_bytes = total_decoded_bytes
                    .checked_add(frame_bytes)
                    .ok_or_else(|| err_limit("GIF total decoded memory overflow"))?;
                if total_decoded_bytes > MAX_MEDIA_BYTES {
                    return Err(err_limit(
                        "GIF total decoded frames exceed 64 MiB limit",
                    ));
                }
                if pos + 9 > data.len() {
                    return Err(err_corrupt("Truncated GIF image descriptor"));
                }
                let fx = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
                let fy = u16::from_le_bytes([data[pos + 2], data[pos + 3]]) as usize;
                let fw = u16::from_le_bytes([data[pos + 4], data[pos + 5]]) as usize;
                let fh = u16::from_le_bytes([data[pos + 6], data[pos + 7]]) as usize;
                let fpacked = data[pos + 8];
                pos += 9;

                if fw == 0 || fh == 0 || fx.saturating_add(fw) > w || fy.saturating_add(fh) > h {
                    return Err(err_corrupt("GIF frame bounds exceed logical screen"));
                }

                let lct = if (fpacked & 0x80) != 0 {
                    let count = 1usize << ((fpacked & 0x07) as usize + 1);
                    let bytes = count * 3;
                    if pos + bytes > data.len() {
                        return Err(err_corrupt("Truncated GIF local color table"));
                    }
                    let mut pal = Vec::with_capacity(count);
                    for chunk in data[pos..pos + bytes].chunks_exact(3) {
                        pal.push([chunk[0], chunk[1], chunk[2]]);
                    }
                    pos += bytes;
                    Some(pal)
                } else {
                    None
                };
                let palette = lct
                    .as_deref()
                    .or(gct.as_deref())
                    .ok_or_else(|| err_corrupt("GIF frame has neither local nor global color table"))?;

                if pos >= data.len() {
                    return Err(err_corrupt("Truncated GIF LZW minimum code size"));
                }
                let min_code_size = data[pos];
                pos += 1;
                let (compressed, next_pos) = read_sub_blocks(data, pos)?;
                pos = next_pos;

                let frame_px = fw * fh;
                let indices = lzw_decode(min_code_size, &compressed, frame_px, deadline_check)?;
                let interlaced = (fpacked & 0x40) != 0;

                let prev_canvas = if gce_disposal == 3 {
                    Some(canvas.clone())
                } else {
                    None
                };

                if !interlaced {
                    for ly in 0..fh {
                        let dst_y = fy + ly;
                        for lx in 0..fw {
                            let idx = indices[ly * fw + lx];
                            if gce_trans_idx == Some(idx) {
                                continue;
                            }
                            let rgb = palette.get(idx as usize).copied().unwrap_or([0, 0, 0]);
                            let di = (dst_y * w + (fx + lx)) * 4;
                            canvas[di] = rgb[0];
                            canvas[di + 1] = rgb[1];
                            canvas[di + 2] = rgb[2];
                            canvas[di + 3] = 255;
                        }
                    }
                } else {
                    const PASSES: [(usize, usize); 4] = [(0, 8), (4, 8), (2, 4), (1, 2)];
                    let mut row_i = 0usize;
                    for &(start, step) in &PASSES {
                        let mut ly = start;
                        while ly < fh {
                            let dst_y = fy + ly;
                            for lx in 0..fw {
                                let idx = indices[row_i * fw + lx];
                                if gce_trans_idx != Some(idx) {
                                    let rgb =
                                        palette.get(idx as usize).copied().unwrap_or([0, 0, 0]);
                                    let di = (dst_y * w + (fx + lx)) * 4;
                                    canvas[di] = rgb[0];
                                    canvas[di + 1] = rgb[1];
                                    canvas[di + 2] = rgb[2];
                                    canvas[di + 3] = 255;
                                }
                            }
                            row_i += 1;
                            ly += step;
                        }
                    }
                }

                frames.push(canvas.clone());
                delays_ms.push(gce_delay_ms);

                match gce_disposal {
                    2 => {
                        for ly in 0..fh {
                            let dst_y = fy + ly;
                            for lx in 0..fw {
                                let di = (dst_y * w + (fx + lx)) * 4;
                                canvas[di..di + 4].copy_from_slice(&[0, 0, 0, 0]);
                            }
                        }
                    }
                    3 => {
                        if let Some(prev) = prev_canvas {
                            canvas = prev;
                        }
                    }
                    _ => {}
                }

                gce_disposal = 0;
                gce_trans_idx = None;
                gce_delay_ms = 100;
            }
            other => {
                return Err(err_unsupported(format!(
                    "Unsupported GIF block introducer 0x{:02X}",
                    other
                )));
            }
        }
    }

    if frames.is_empty() {
        return Err(err_corrupt("GIF stream contains zero image frames"));
    }

    let first = frames[0].clone();
    let multi = if frames.len() > 1 { Some(frames) } else { None };
    Ok(RawDecodedImage {
        width,
        height,
        format: PixelFormat::Rgba8,
        source: SourceFormat::Gif,
        data: first,
        frames: multi,
        delays_ms,
        loop_count,
    })
}
