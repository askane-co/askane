use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::formats::RawDecodedImage;
use crate::media::image::{PixelFormat, SourceFormat};
use crate::media::{
    check_deadline, err_corrupt, err_limit, err_unsupported, temp_budget_guard, validate_dims,
    MAX_DIM, MAX_MEDIA_BYTES, MAX_PIXELS,
};

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
    59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone)]
struct HuffTable {
    maxcode: [i32; 17],
    valoffset: [i32; 17],
    values: Vec<u8>,
    look_nbits: [u8; 256],
    look_sym: [u8; 256],
}

impl HuffTable {
    fn build(bits: &[u8; 16], vals: &[u8]) -> Result<Self, String> {
        // Validate canonical Huffman Kraft space against oversubscription.
        let mut space: i32 = 1 << 16;
        let mut total = 0usize;
        for (i, &cnt) in bits.iter().enumerate() {
            let c = cnt as i32;
            total += cnt as usize;
            let weight = 1 << (15 - i);
            space -= c * weight;
            if space < 0 {
                return Err(err_corrupt("Oversubscribed JPEG Huffman table"));
            }
        }
        if total > 256 || total != vals.len() {
            return Err(err_corrupt("JPEG Huffman count/symbol length mismatch"));
        }

        let mut huffsize = Vec::with_capacity(total + 1);
        for (i, &cnt) in bits.iter().enumerate() {
            for _ in 0..cnt {
                huffsize.push((i + 1) as u8);
            }
        }
        let mut huffcode = vec![0u32; total];
        let mut code = 0u32;
        let mut si = huffsize.first().copied().unwrap_or(0);
        let mut k = 0usize;
        while k < total {
            while k < total && huffsize[k] == si {
                huffcode[k] = code;
                code += 1;
                k += 1;
            }
            if k < total {
                while si < huffsize[k] {
                    code <<= 1;
                    si += 1;
                }
            }
        }

        let mut maxcode = [-1i32; 17];
        let mut valoffset = [0i32; 17];
        let mut p = 0usize;
        for l in 1..=16usize {
            let cnt = bits[l - 1] as usize;
            if cnt > 0 {
                valoffset[l] = (p as i32) - (huffcode[p] as i32);
                p += cnt;
                maxcode[l] = huffcode[p - 1] as i32;
            } else {
                maxcode[l] = -1;
            }
        }

        let mut look_nbits = [0u8; 256];
        let mut look_sym = [0u8; 256];
        p = 0;
        for l in 1..=8usize {
            for _ in 0..bits[l - 1] {
                let lookbits = (huffcode[p] << (8 - l)) as usize;
                let fill = 1usize << (8 - l);
                for offset in 0..fill {
                    let idx = lookbits + offset;
                    if idx < 256 {
                        look_nbits[idx] = l as u8;
                        look_sym[idx] = vals[p];
                    }
                }
                p += 1;
            }
        }

        Ok(Self {
            maxcode,
            valoffset,
            values: vals.to_vec(),
            look_nbits,
            look_sym,
        })
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    bits: u8,
    eof_hit: bool,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], start: usize) -> Self {
        Self {
            data,
            pos: start,
            buf: 0,
            bits: 0,
            eof_hit: false,
        }
    }

    fn fill(&mut self, needed: u8) -> Result<(), String> {
        while self.bits < needed {
            if self.pos >= self.data.len() {
                self.eof_hit = true;
                self.buf <<= 8;
                self.bits += 8;
                continue;
            }
            let b = self.data[self.pos];
            self.pos += 1;
            let byte = if b == 0xFF {
                if self.pos < self.data.len() && self.data[self.pos] == 0x00 {
                    self.pos += 1;
                    0xFF
                } else {
                    self.pos -= 1;
                    self.eof_hit = true;
                    0
                }
            } else {
                b
            };
            self.buf = (self.buf << 8) | (byte as u32);
            self.bits += 8;
        }
        Ok(())
    }

    #[inline]
    fn get_bits(&mut self, n: u8) -> Result<i32, String> {
        if n == 0 {
            return Ok(0);
        }
        self.fill(n)?;
        if self.eof_hit && self.bits < n {
            return Err(err_corrupt("Unexpected EOF inside JPEG entropy stream"));
        }
        self.bits -= n;
        let v = ((self.buf >> self.bits) & ((1u32 << n) - 1)) as i32;
        Ok(v)
    }

    fn decode_huff(&mut self, ht: &HuffTable) -> Result<u8, String> {
        self.fill(8)?;
        let peek = ((self.buf >> (self.bits - 8)) & 0xFF) as usize;
        let nb = ht.look_nbits[peek];
        if nb > 0 {
            if self.eof_hit && self.bits < nb {
                return Err(err_corrupt("Truncated JPEG Huffman symbol"));
            }
            self.bits -= nb;
            return Ok(ht.look_sym[peek]);
        }
        let mut code = ((self.buf >> (self.bits - 8)) & 0xFF) as i32;
        self.bits -= 8;
        for l in 9..=16usize {
            let b = self.get_bits(1)?;
            code = (code << 1) | b;
            if ht.maxcode[l] >= 0 && code <= ht.maxcode[l] {
                let idx = (code + ht.valoffset[l]) as usize;
                return ht
                    .values
                    .get(idx)
                    .copied()
                    .ok_or_else(|| err_corrupt("JPEG Huffman symbol index out of range"));
            }
        }
        Err(err_corrupt("Invalid JPEG Huffman code"))
    }

    fn expect_restart(&mut self, rst_idx: u8) -> Result<(), String> {
        self.bits = 0;
        self.buf = 0;
        self.eof_hit = false;
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF {
                let m = self.data[self.pos + 1];
                if m == 0x00 {
                    self.pos += 2;
                    continue;
                }
                if m == 0xFF {
                    self.pos += 1;
                    continue;
                }
                if m == 0xD0 + (rst_idx & 7) {
                    self.pos += 2;
                    return Ok(());
                }
                if (0xD0..=0xD7).contains(&m) {
                    self.pos += 2;
                    return Ok(());
                }
                return Err(err_corrupt("Missing expected JPEG RST marker"));
            }
            self.pos += 1;
        }
        Err(err_corrupt("Truncated JPEG before RST marker"))
    }
}

#[inline]
fn extend_sign(v: i32, nbits: u8) -> i32 {
    if nbits == 0 {
        return 0;
    }
    let thresh = 1i32 << (nbits - 1);
    if v < thresh {
        v - ((1i32 << nbits) - 1)
    } else {
        v
    }
}

/// Deterministic fixed-point integer 1D 8-point IDCT (Loeffler-style scaled constants, zero floats).
#[inline]
fn idct_1d(src: &[i32; 8], dst: &mut [i32; 8], pass2: bool) {
    const W1: i32 = 2841; // 2048*sqrt(2)*cos(1*pi/16)
    const W2: i32 = 2676; // 2048*sqrt(2)*cos(2*pi/16)
    const W3: i32 = 2408; // 2048*sqrt(2)*cos(3*pi/16)
    const W5: i32 = 1609; // 2048*sqrt(2)*cos(5*pi/16)
    const W6: i32 = 1108; // 2048*sqrt(2)*cos(6*pi/16)
    const W7: i32 = 565;  // 2048*sqrt(2)*cos(7*pi/16)
    const W1_W7: i32 = W1 + W7;
    const W1_M7: i32 = W1 - W7;
    const W3_W5: i32 = W3 + W5;
    const W3_M5: i32 = W3 - W5;
    const W2_W6: i32 = W2 + W6;
    const W2_M6: i32 = W2 - W6;

    if src[1] == 0
        && src[2] == 0
        && src[3] == 0
        && src[4] == 0
        && src[5] == 0
        && src[6] == 0
        && src[7] == 0
    {
        let dc = if pass2 {
            ((src[0] + 1024) >> 11).clamp(-1024, 1024)
        } else {
            src[0] << 3
        };
        dst.fill(dc);
        return;
    }

    let mut x0 = (src[0] << 11) + 128;
    let mut x1 = src[4] << 11;
    let mut x2 = src[6];
    let mut x3 = src[2];
    let mut x4 = src[1];
    let mut x5 = src[7];
    let mut x6 = src[5];
    let mut x7 = src[3];

    if pass2 {
        x0 = (src[0] << 8) + 8192;
        x1 = src[4] << 8;
        x2 = src[6];
        x3 = src[2];
        x4 = src[1];
        x5 = src[7];
        x6 = src[5];
        x7 = src[3];
    }

    // Even part (pass1 scales by <<11, pass2 scales by <<8 to stay strictly in i32)
    let shift = if pass2 { 14 } else { 8 };
    let _ = W1_W7;
    let _ = W1_M7;
    let _ = W3_W5;
    let _ = W3_M5;

    let stage1 = if pass2 {
        let _ = x1;
        // Reuse uniform integer scale regardless of pass using exact pass factors:
        false
    } else {
        true
    };
    let _ = stage1;

    // Unified orthogonal integer cosine factors scaled by 4096 (12 fractional bits).
    // C[k][x] = norm(k) * cos((2x+1)*k*pi/16) * 4096, deterministic i32 matrix multiply.
    const K: [[i32; 8]; 8] = [
        [1448, 1448, 1448, 1448, 1448, 1448, 1448, 1448],
        [2009, 1703, 1138, 399, -399, -1138, -1703, -2009],
        [1892, 784, -784, -1892, -1892, -784, 784, 1892],
        [1703, -399, -2009, -1138, 1138, 2009, 399, -1703],
        [1448, -1448, -1448, 1448, 1448, -1448, -1448, 1448],
        [1138, -2009, 399, 1703, -1703, -399, 2009, -1138],
        [784, -1892, 1892, -784, -784, 1892, -1892, 784],
        [399, -1138, 1703, -2009, 2009, -1703, 1138, -399],
    ];
    let _ = (W2_W6, W2_M6, x0, x2, x3, x4, x5, x6, x7, shift);

    for x in 0..8 {
        let mut sum: i32 = 0;
        for k in 0..8 {
            sum += src[k] * K[k][x];
        }
        dst[x] = if pass2 {
            (sum + (1 << 15)) >> 16
        } else {
            (sum + (1 << 7)) >> 8
        };
    }
}

fn idct_block(coeffs: &[i32; 64], out: &mut [u8; 64]) {
    let mut inter = [0i32; 64];
    for r in 0..8 {
        let row_src: [i32; 8] = coeffs[r * 8..r * 8 + 8].try_into().unwrap();
        let mut row_dst = [0i32; 8];
        idct_1d(&row_src, &mut row_dst, false);
        inter[r * 8..r * 8 + 8].copy_from_slice(&row_dst);
    }
    for c in 0..8 {
        let col_src = [
            inter[c],
            inter[8 + c],
            inter[16 + c],
            inter[24 + c],
            inter[32 + c],
            inter[40 + c],
            inter[48 + c],
            inter[56 + c],
        ];
        let mut col_dst = [0i32; 8];
        idct_1d(&col_src, &mut col_dst, true);
        for r in 0..8 {
            out[r * 8 + c] = (col_dst[r] + 128).clamp(0, 255) as u8;
        }
    }
}

#[derive(Clone, Copy)]
struct FrameComp {
    id: u8,
    h: usize,
    v: usize,
    qt_idx: usize,
}

pub fn decode_jpeg(
    data: &[u8],
    budget: Option<&Arc<BudgetCore>>,
    deadline_check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<RawDecodedImage, String> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(err_corrupt("Invalid JPEG SOI marker"));
    }

    let mut qt: [Option<[i32; 64]>; 4] = [None, None, None, None];
    let mut huff_dc: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut huff_ac: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut restart_interval: usize = 0;

    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut comps: Vec<FrameComp> = Vec::new();
    let mut saw_sof = false;
    let mut pos = 2usize;
    let mut scan_start = None;
    let mut scan_selectors: Vec<(usize, usize, usize)> = Vec::new();

    while pos + 1 < data.len() {
        check_deadline(deadline_check)?;
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        let marker = data[pos];
        pos += 1;

        if marker == 0xD9 {
            break;
        }
        if marker == 0x00 || marker == 0xD8 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if pos + 2 > data.len() {
            return Err(err_corrupt("Truncated JPEG marker length"));
        }
        let seg_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > data.len() {
            return Err(err_corrupt("Invalid JPEG segment length"));
        }
        let seg = &data[pos + 2..pos + seg_len];
        pos += seg_len;

        match marker {
            0xC0 | 0xC1 => {
                if saw_sof || seg.len() < 6 {
                    return Err(err_corrupt("Duplicate or invalid JPEG SOF"));
                }
                let precision = seg[0];
                if precision != 8 {
                    return Err(err_unsupported(format!(
                        "Unsupported JPEG sample precision {} (expected 8)",
                        precision
                    )));
                }
                height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
                width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                let _ = validate_dims(width, height, MAX_DIM)?;
                let nf = seg[5] as usize;
                if nf != 1 && nf != 3 {
                    return Err(err_unsupported(format!(
                        "Unsupported JPEG component count {} (only Gray=1 and YCbCr=3 supported)",
                        nf
                    )));
                }
                if seg.len() != 6 + nf * 3 {
                    return Err(err_corrupt("JPEG SOF component table truncated"));
                }
                comps.clear();
                let mut hv_sum = 0usize;
                for c in 0..nf {
                    let id = seg[6 + c * 3];
                    let hv = seg[6 + c * 3 + 1];
                    let h = ((hv >> 4) & 0x0F) as usize;
                    let v = (hv & 0x0F) as usize;
                    let qt_idx = seg[6 + c * 3 + 2] as usize;
                    if h == 0 || v == 0 || h > 4 || v > 4 || qt_idx >= 4 {
                        return Err(err_corrupt("Invalid JPEG component sampling or DQT id"));
                    }
                    hv_sum += h * v;
                    comps.push(FrameComp { id, h, v, qt_idx });
                }
                if hv_sum > 10 {
                    return Err(err_limit("JPEG MCU component blocks sum exceeds 10"));
                }
                saw_sof = true;
            }
            0xC2 => {
                return Err(err_unsupported(
                    "Progressive JPEG (SOF2) is not supported in 0.20 (baseline/sequential SOF0/SOF1 only)",
                ));
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(err_unsupported(format!(
                    "Unsupported JPEG frame mode SOF marker 0xFF{:02X}",
                    marker
                )));
            }
            0xDB => {
                let mut sp = 0usize;
                while sp < seg.len() {
                    let pq_tq = seg[sp];
                    sp += 1;
                    let pq = pq_tq >> 4;
                    let tq = (pq_tq & 0x0F) as usize;
                    if tq >= 4 || pq > 1 {
                        return Err(err_corrupt("Invalid JPEG DQT index or precision"));
                    }
                    let mut table = [0i32; 64];
                    if pq == 0 {
                        if sp + 64 > seg.len() {
                            return Err(err_corrupt("Truncated JPEG DQT 8-bit table"));
                        }
                        for i in 0..64 {
                            table[ZIGZAG[i]] = seg[sp + i] as i32;
                        }
                        sp += 64;
                    } else {
                        if sp + 128 > seg.len() {
                            return Err(err_corrupt("Truncated JPEG DQT 16-bit table"));
                        }
                        for i in 0..64 {
                            table[ZIGZAG[i]] = u16::from_be_bytes([seg[sp + i * 2], seg[sp + i * 2 + 1]]) as i32;
                        }
                        sp += 128;
                    }
                    qt[tq] = Some(table);
                }
            }
            0xC4 => {
                let mut sp = 0usize;
                while sp < seg.len() {
                    if sp + 17 > seg.len() {
                        return Err(err_corrupt("Truncated JPEG DHT header"));
                    }
                    let tc_th = seg[sp];
                    sp += 1;
                    let tc = tc_th >> 4;
                    let th = (tc_th & 0x0F) as usize;
                    if tc > 1 || th >= 4 {
                        return Err(err_corrupt("Invalid JPEG DHT class or id"));
                    }
                    let mut bits = [0u8; 16];
                    bits.copy_from_slice(&seg[sp..sp + 16]);
                    sp += 16;
                    let sym_count: usize = bits.iter().map(|&b| b as usize).sum();
                    if sp + sym_count > seg.len() {
                        return Err(err_corrupt("Truncated JPEG DHT values"));
                    }
                    let ht = HuffTable::build(&bits, &seg[sp..sp + sym_count])?;
                    sp += sym_count;
                    if tc == 0 {
                        huff_dc[th] = Some(ht);
                    } else {
                        huff_ac[th] = Some(ht);
                    }
                }
            }
            0xDD => {
                if seg.len() != 2 {
                    return Err(err_corrupt("Invalid JPEG DRI segment"));
                }
                restart_interval = u16::from_be_bytes([seg[0], seg[1]]) as usize;
            }
            0xDA => {
                if !saw_sof || seg.is_empty() {
                    return Err(err_corrupt("JPEG SOS before SOF or empty SOS"));
                }
                let ns = seg[0] as usize;
                if ns != comps.len() || seg.len() != 1 + ns * 2 + 3 {
                    return Err(err_unsupported("Non-interleaved or mismatched JPEG SOS scan"));
                }
                if seg[1 + ns * 2] != 0 || seg[2 + ns * 2] != 63 || seg[3 + ns * 2] != 0 {
                    return Err(err_unsupported("Unsupported progressive/spectral JPEG SOS range"));
                }
                scan_selectors.clear();
                for i in 0..ns {
                    let cid = seg[1 + i * 2];
                    let td_ta = seg[2 + i * 2];
                    let td = (td_ta >> 4) as usize;
                    let ta = (td_ta & 0x0F) as usize;
                    let comp_idx = comps
                        .iter()
                        .position(|c| c.id == cid)
                        .ok_or_else(|| err_corrupt("JPEG SOS unknown component id"))?;
                    if td >= 4 || ta >= 4 {
                        return Err(err_corrupt("JPEG SOS Huffman index out of range"));
                    }
                    scan_selectors.push((comp_idx, td, ta));
                }
                scan_start = Some(pos);
                break;
            }
            _ => {}
        }
    }

    if !saw_sof {
        return Err(err_corrupt("JPEG missing SOF0/SOF1 header"));
    }
    let entropy_start = scan_start.ok_or_else(|| err_corrupt("JPEG missing SOS scan data"))?;

    for (c_idx, td, ta) in &scan_selectors {
        if qt[comps[*c_idx].qt_idx].is_none() || huff_dc[*td].is_none() || huff_ac[*ta].is_none() {
            return Err(err_corrupt("JPEG missing DQT or DHT table referenced by scan"));
        }
    }

    let max_h = comps.iter().map(|c| c.h).max().unwrap_or(1);
    let max_v = comps.iter().map(|c| c.v).max().unwrap_or(1);
    let mcu_w = max_h * 8;
    let mcu_h = max_v * 8;
    let mcus_x = (width as usize).div_ceil(mcu_w);
    let mcus_y = (height as usize).div_ceil(mcu_h);
    let total_mcus = mcus_x
        .checked_mul(mcus_y)
        .ok_or_else(|| err_limit("JPEG MCU count overflow"))?;
    if total_mcus > MAX_PIXELS / 64 + 8192 {
        return Err(err_limit("JPEG MCU count exceeds decoded pixel limit"));
    }

    let out_fmt = if comps.len() == 1 {
        PixelFormat::R8
    } else {
        PixelFormat::Rgb8
    };
    let out_bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(out_fmt.bpp()))
        .ok_or_else(|| err_limit("JPEG output buffer overflow"))?;
    if out_bytes > MAX_MEDIA_BYTES {
        return Err(err_limit("Decoded JPEG exceeds 64 MiB limit"));
    }

    let mcu_scratch_bytes = mcu_w * mcu_h * comps.len();
    let _guard = temp_budget_guard(budget, mcu_scratch_bytes)?;

    let mut out = vec![0u8; out_bytes];
    let mut reader = BitReader::new(data, entropy_start);
    let mut dc_pred = [0i32; 4];
    let mut mcu_buf = vec![0u8; mcu_scratch_bytes];
    let mut rst_counter = 0u8;
    let w = width as usize;
    let h = height as usize;

    for mcu_idx in 0..total_mcus {
        if (mcu_idx & 255) == 0 {
            check_deadline(deadline_check)?;
        }
        if restart_interval > 0 && mcu_idx > 0 && (mcu_idx % restart_interval) == 0 {
            reader.expect_restart(rst_counter)?;
            rst_counter = (rst_counter + 1) & 7;
            dc_pred = [0i32; 4];
        }

        let mcu_x0 = (mcu_idx % mcus_x) * mcu_w;
        let mcu_y0 = (mcu_idx / mcus_x) * mcu_h;

        for (comp_order, &(c_idx, td, ta)) in scan_selectors.iter().enumerate() {
            let comp = comps[c_idx];
            let qtable = qt[comp.qt_idx].as_ref().unwrap();
            let dc_ht = huff_dc[td].as_ref().unwrap();
            let ac_ht = huff_ac[ta].as_ref().unwrap();
            let comp_plane_offset = comp_order * mcu_w * mcu_h;

            for v_blk in 0..comp.v {
                for h_blk in 0..comp.h {
                    let mut coeffs = [0i32; 64];
                    let cat = reader.decode_huff(dc_ht)? & 0x0F;
                    if cat > 11 {
                        return Err(err_corrupt("JPEG DC category > 11"));
                    }
                    let bits = reader.get_bits(cat)?;
                    dc_pred[c_idx] += extend_sign(bits, cat);
                    coeffs[0] = dc_pred[c_idx] * qtable[0];

                    let mut k = 1usize;
                    while k < 64 {
                        let rs = reader.decode_huff(ac_ht)?;
                        let r = (rs >> 4) as usize;
                        let s = rs & 0x0F;
                        if s == 0 {
                            if r == 15 {
                                k += 16;
                            } else {
                                break;
                            }
                        } else {
                            k += r;
                            if k >= 64 {
                                return Err(err_corrupt("JPEG AC coefficient index > 63"));
                            }
                            if s > 11 {
                                return Err(err_corrupt("JPEG AC category > 11"));
                            }
                            let abits = reader.get_bits(s)?;
                            coeffs[ZIGZAG[k]] = extend_sign(abits, s) * qtable[ZIGZAG[k]];
                            k += 1;
                        }
                    }

                    let mut block_px = [0u8; 64];
                    idct_block(&coeffs, &mut block_px);

                    let scale_x = max_h / comp.h;
                    let scale_y = max_v / comp.v;
                    let bx0 = h_blk * 8 * scale_x;
                    let by0 = v_blk * 8 * scale_y;
                    for py in 0..8 {
                        for px in 0..8 {
                            let val = block_px[py * 8 + px];
                            for sy in 0..scale_y {
                                let ly = by0 + py * scale_y + sy;
                                for sx in 0..scale_x {
                                    let lx = bx0 + px * scale_x + sx;
                                    mcu_buf[comp_plane_offset + ly * mcu_w + lx] = val;
                                }
                            }
                        }
                    }
                }
            }
        }

        let plane_sz = mcu_w * mcu_h;
        let max_lx = (w - mcu_x0).min(mcu_w);
        let max_ly = (h - mcu_y0).min(mcu_h);
        if comps.len() == 1 {
            for ly in 0..max_ly {
                let py = mcu_y0 + ly;
                for lx in 0..max_lx {
                    let px = mcu_x0 + lx;
                    out[py * w + px] = mcu_buf[ly * mcu_w + lx];
                }
            }
        } else {
            for ly in 0..max_ly {
                let py = mcu_y0 + ly;
                for lx in 0..max_lx {
                    let px = mcu_x0 + lx;
                    let si = ly * mcu_w + lx;
                    let yv = mcu_buf[si] as i32;
                    let cb = mcu_buf[plane_sz + si] as i32 - 128;
                    let cr = mcu_buf[plane_sz * 2 + si] as i32 - 128;
                    let r = (yv + ((359 * cr + 128) >> 8)).clamp(0, 255) as u8;
                    let g = (yv - ((88 * cb + 183 * cr + 128) >> 8)).clamp(0, 255) as u8;
                    let b = (yv + ((454 * cb + 128) >> 8)).clamp(0, 255) as u8;
                    let di = (py * w + px) * 3;
                    out[di] = r;
                    out[di + 1] = g;
                    out[di + 2] = b;
                }
            }
        }
    }

    Ok(RawDecodedImage {
        width,
        height,
        format: out_fmt,
        source: SourceFormat::Jpeg,
        data: out,
        frames: None,
        delays_ms: Vec::new(),
        loop_count: 0,
    })
}
