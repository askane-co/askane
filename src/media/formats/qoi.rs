use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::formats::RawDecodedImage;
use crate::media::image::{PixelFormat, SourceFormat};
use crate::media::{check_deadline, err_corrupt, err_limit, err_unsupported, validate_dims, MAX_DIM, MAX_MEDIA_BYTES};

const QOI_END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];

pub fn decode_qoi(
    data: &[u8],
    _budget: Option<&Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<RawDecodedImage, String> {
    if data.len() < 14 + 8 || &data[0..4] != b"qoif" {
        return Err(err_corrupt("Invalid QOI header"));
    }
    let width = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let height = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let channels = data[12];
    let colorspace = data[13];
    if channels != 3 && channels != 4 {
        return Err(err_unsupported(format!("Unsupported QOI channels {}", channels)));
    }
    if colorspace > 1 {
        return Err(err_unsupported(format!(
            "Unsupported QOI colorspace {}",
            colorspace
        )));
    }
    let pixels = validate_dims(width, height, MAX_DIM)?;
    let out_fmt = if channels == 4 {
        PixelFormat::Rgba8
    } else {
        PixelFormat::Rgb8
    };
    let bpp = out_fmt.bpp();
    let total_bytes = pixels
        .checked_mul(bpp)
        .ok_or_else(|| err_limit("QOI output buffer overflow"))?;
    if total_bytes > MAX_MEDIA_BYTES {
        return Err(err_limit("Decoded QOI image exceeds 64 MiB limit"));
    }

    let mut out = vec![0u8; total_bytes];
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0u8, 0u8, 255u8];
    let mut pos = 14usize;
    let stream_end = data.len() - 8;
    let mut idx = 0usize;

    while idx < pixels {
        if (idx & 65535) == 0 {
            check_deadline(deadline_check)?;
        }
        if pos >= stream_end {
            return Err(err_corrupt("Truncated QOI pixel stream"));
        }
        let tag = data[pos];
        pos += 1;
        let mut run = 1usize;
        if tag == 0xFE {
            if pos + 3 > stream_end {
                return Err(err_corrupt("Truncated QOI RGB opcode"));
            }
            px[0] = data[pos];
            px[1] = data[pos + 1];
            px[2] = data[pos + 2];
            pos += 3;
        } else if tag == 0xFF {
            if pos + 4 > stream_end {
                return Err(err_corrupt("Truncated QOI RGBA opcode"));
            }
            px = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
            pos += 4;
        } else {
            match tag & 0xC0 {
                0x00 => {
                    px = index[(tag & 0x3F) as usize];
                }
                0x40 => {
                    let dr = ((tag >> 4) & 0x03).wrapping_sub(2);
                    let dg = ((tag >> 2) & 0x03).wrapping_sub(2);
                    let db = (tag & 0x03).wrapping_sub(2);
                    px[0] = px[0].wrapping_add(dr);
                    px[1] = px[1].wrapping_add(dg);
                    px[2] = px[2].wrapping_add(db);
                }
                0x80 => {
                    if pos >= stream_end {
                        return Err(err_corrupt("Truncated QOI LUMA opcode"));
                    }
                    let b2 = data[pos];
                    pos += 1;
                    let vg = (tag & 0x3F).wrapping_sub(32);
                    let dr = vg.wrapping_add(((b2 >> 4) & 0x0F).wrapping_sub(8));
                    let db = vg.wrapping_add((b2 & 0x0F).wrapping_sub(8));
                    px[0] = px[0].wrapping_add(dr);
                    px[1] = px[1].wrapping_add(vg);
                    px[2] = px[2].wrapping_add(db);
                }
                0xC0 => {
                    run = ((tag & 0x3F) as usize) + 1;
                }
                _ => unreachable!(),
            }
        }
        if idx + run > pixels {
            return Err(err_corrupt("QOI stream overruns image pixel count"));
        }
        let hash_idx = ((px[0] as usize) * 3
            + (px[1] as usize) * 5
            + (px[2] as usize) * 7
            + (px[3] as usize) * 11)
            & 63;
        index[hash_idx] = px;

        for k in 0..run {
            let di = (idx + k) * bpp;
            out[di] = px[0];
            out[di + 1] = px[1];
            out[di + 2] = px[2];
            if bpp == 4 {
                out[di + 3] = px[3];
            }
        }
        idx += run;
    }

    if pos != stream_end || data[stream_end..] != QOI_END {
        return Err(err_corrupt("Invalid QOI end marker or trailing stream bytes"));
    }

    Ok(RawDecodedImage {
        width,
        height,
        format: out_fmt,
        source: SourceFormat::Qoi,
        data: out,
        frames: None,
        delays_ms: Vec::new(),
        loop_count: 0,
    })
}
