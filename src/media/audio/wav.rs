use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::audio::{ENGINE_RATE, MAX_SOUND_BYTES, err_stream_unsupported};
use crate::media::audio::sound::SoundData;
use crate::media::{
    Digest, MAX_SOURCE_BYTES, check_deadline, err_bad_opt, err_corrupt, err_limit, err_unsupported,
    temp_budget_guard,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavFormat {
    PcmU8,
    PcmS16,
    PcmS24,
    PcmS32,
    IeeeF32,
    IeeeF64,
    Alaw,
    Mulaw,
    ImaAdpcm {
        block_align: u16,
        samples_per_block: u16,
    },
}

#[derive(Clone, Debug)]
pub struct WavMeta {
    pub format: WavFormat,
    pub sample_rate: u32,
    pub channels: u16,
    pub data_offset: usize,
    pub data_len: usize,
    pub loop_points: Option<(u64, u64)>,
}

const ALAW_TABLE_SIGN: i16 = 0x80;
const ALAW_QUANT_MASK: u8 = 0x0F;
const ALAW_SEG_SHIFT: u8 = 4;
const ALAW_SEG_MASK: u8 = 0x70;

fn decode_alaw_sample(a_val: u8) -> f32 {
    let val = a_val ^ 0x55;
    let mut t = ((val & ALAW_QUANT_MASK) as i32) << 4;
    let seg = ((val & ALAW_SEG_MASK) >> ALAW_SEG_SHIFT) as i32;
    match seg {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= seg - 1;
        }
    }
    let pcm = if (val & (ALAW_SIGN_BIT as u8)) != 0 {
        t
    } else {
        -t
    };
    (pcm as f32) / 32768.0
}

const ALAW_SIGN_BIT: i16 = ALAW_TABLE_SIGN;

fn decode_mulaw_sample(u_val: u8) -> f32 {
    let u = !u_val;
    let sign = u & 0x80;
    let exponent = (u >> 4) & 0x07;
    let mantissa = u & 0x0F;
    let mut sample = (((mantissa as i32) << 3) + 0x84) << exponent;
    sample -= 0x84;
    let pcm = if sign != 0 { -sample } else { sample };
    (pcm as f32) / 32768.0
}

const IMA_INDEX_TABLE: [i8; 16] = [
    -1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8,
];

const IMA_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

#[inline]
fn ima_step(nibble: u8, predictor: &mut i32, step_index: &mut i32) -> f32 {
    let step = IMA_STEP_TABLE[(*step_index).clamp(0, 88) as usize];
    let mut diff = step >> 3;
    if (nibble & 1) != 0 {
        diff += step >> 2;
    }
    if (nibble & 2) != 0 {
        diff += step >> 1;
    }
    if (nibble & 4) != 0 {
        diff += step;
    }
    if (nibble & 8) != 0 {
        *predictor -= diff;
    } else {
        *predictor += diff;
    }
    *predictor = (*predictor).clamp(-32768, 32767);
    *step_index = (*step_index + IMA_INDEX_TABLE[(nibble & 0x0F) as usize] as i32).clamp(0, 88);
    (*predictor as f32) / 32768.0
}

pub fn parse_wav_header(data: &[u8]) -> Result<WavMeta, String> {
    if data.len() > MAX_SOURCE_BYTES {
        return Err(err_limit(format!(
            "WAV source {} bytes exceeds {} byte limit",
            data.len(),
            MAX_SOURCE_BYTES
        )));
    }
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(err_corrupt("invalid RIFF/WAVE header"));
    }

    let mut pos = 12usize;
    let mut fmt_info: Option<(u16, u16, u32, u16, u16, u16, u16)> = None;
    let mut data_span: Option<(usize, usize)> = None;
    let mut loop_points: Option<(u64, u64)> = None;

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let chunk_size =
            u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
                as usize;
        let body_start = pos + 8;
        let body_end = body_start
            .checked_add(chunk_size)
            .ok_or_else(|| err_corrupt("WAV chunk size overflow"))?;
        if body_end > data.len() {
            return Err(err_corrupt("truncated WAV chunk exceeds EOF"));
        }
        let body = &data[body_start..body_end];

        if id == b"fmt " {
            if body.len() < 16 {
                return Err(err_corrupt("WAV 'fmt ' chunk shorter than 16 bytes"));
            }
            let raw_tag = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            let block_align = u16::from_le_bytes([body[12], body[13]]);
            let bits = u16::from_le_bytes([body[14], body[15]]);

            let mut eff_tag = raw_tag;
            let mut extra_val = 0u16;
            if body.len() >= 18 {
                let cb_size = u16::from_le_bytes([body[16], body[17]]) as usize;
                if raw_tag == 0x0011 && body.len() >= 20 && cb_size >= 2 {
                    extra_val = u16::from_le_bytes([body[18], body[19]]);
                } else if raw_tag == 0xFFFE {
                    if body.len() < 40 || cb_size < 22 {
                        return Err(err_corrupt("WAV WAVE_FORMAT_EXTENSIBLE chunk truncated"));
                    }
                    // Standard Microsoft KSDATAFORMAT_SUBTYPE_*: LE u16 sub-tag at offset 24
                    eff_tag = u16::from_le_bytes([body[24], body[25]]);
                }
            }
            fmt_info = Some((raw_tag, eff_tag, sample_rate, channels, block_align, bits, extra_val));
        } else if id == b"data" {
            if data_span.is_none() {
                data_span = Some((body_start, chunk_size));
            }
        } else if id == b"smpl" && body.len() >= 60 {
            let num_loops = u32::from_le_bytes([body[28], body[29], body[30], body[31]]);
            if num_loops > 0 {
                let start = u32::from_le_bytes([body[44], body[45], body[46], body[47]]) as u64;
                let end = u32::from_le_bytes([body[48], body[49], body[50], body[51]]) as u64;
                if end > start {
                    loop_points = Some((start, end));
                }
            }
        }

        // RIFF chunks are word-aligned (padded by 1 byte if odd size).
        pos = body_end + (chunk_size & 1);
    }

    let (_, tag, sample_rate, channels, block_align, bits, extra_val) =
        fmt_info.ok_or_else(|| err_corrupt("WAV missing 'fmt ' chunk"))?;
    let (data_offset, data_len) =
        data_span.ok_or_else(|| err_corrupt("WAV missing 'data' chunk"))?;

    if channels == 0 || channels > 8 {
        return Err(err_unsupported(format!(
            "WAV unsupported channel count {}",
            channels
        )));
    }
    if sample_rate == 0 || sample_rate > 384_000 {
        return Err(err_corrupt(format!("WAV invalid sample_rate {}", sample_rate)));
    }

    let format = match (tag, bits) {
        (1, 8) => WavFormat::PcmU8,
        (1, 16) => WavFormat::PcmS16,
        (1, 24) => WavFormat::PcmS24,
        (1, 32) => WavFormat::PcmS32,
        (3, 32) => WavFormat::IeeeF32,
        (3, 64) => WavFormat::IeeeF64,
        (6, 8) => WavFormat::Alaw,
        (7, 8) => WavFormat::Mulaw,
        (0x0011, 4) => {
            let min_hdr = 4usize * (channels as usize);
            if (block_align as usize) < min_hdr + 4 || ((block_align as usize - min_hdr) % (4 * channels as usize)) != 0 {
                return Err(err_corrupt(format!(
                    "WAV IMA ADPCM invalid block_align {} for {} channels",
                    block_align, channels
                )));
            }
            if channels > 2 {
                return Err(err_unsupported("IMA ADPCM only supports 1 or 2 channels"));
            }
            let spb = if extra_val > 0 {
                extra_val
            } else {
                let data_bytes = (block_align as usize) - min_hdr;
                (1 + (data_bytes * 2) / (channels as usize)) as u16
            };
            WavFormat::ImaAdpcm {
                block_align,
                samples_per_block: spb,
            }
        }
        _ => {
            return Err(err_unsupported(format!(
                "WAV unsupported audio format tag {} ({} bits)",
                tag, bits
            )));
        }
    };

    Ok(WavMeta {
        format,
        sample_rate,
        channels,
        data_offset,
        data_len,
        loop_points,
    })
}

pub fn decode_wav_chunk_to_stereo(meta: &WavMeta, raw: &[u8]) -> Result<Vec<f32>, String> {
    let ch = meta.channels as usize;
    match meta.format {
        WavFormat::PcmU8
        | WavFormat::PcmS16
        | WavFormat::PcmS24
        | WavFormat::PcmS32
        | WavFormat::IeeeF32
        | WavFormat::IeeeF64
        | WavFormat::Alaw
        | WavFormat::Mulaw => {
            let bps = match meta.format {
                WavFormat::PcmU8 | WavFormat::Alaw | WavFormat::Mulaw => 1usize,
                WavFormat::PcmS16 => 2,
                WavFormat::PcmS24 => 3,
                WavFormat::PcmS32 | WavFormat::IeeeF32 => 4,
                WavFormat::IeeeF64 => 8,
                _ => unreachable!(),
            };
            let frame_bytes = ch * bps;
            let num_frames = raw.len() / frame_bytes;
            let mut out = Vec::with_capacity(num_frames * 2);
            let read_one = |s: &[u8]| -> f32 {
                match meta.format {
                    WavFormat::PcmU8 => (s[0] as f32 - 128.0) / 128.0,
                    WavFormat::PcmS16 => {
                        let v = i16::from_le_bytes([s[0], s[1]]);
                        (v as f32) / 32768.0
                    }
                    WavFormat::PcmS24 => {
                        let sign = if (s[2] & 0x80) != 0 { 0xFF } else { 0 };
                        let v = i32::from_le_bytes([s[0], s[1], s[2], sign]);
                        (v as f32) / 8_388_608.0
                    }
                    WavFormat::PcmS32 => {
                        let v = i32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                        ((v as f64) / 2_147_483_648.0) as f32
                    }
                    WavFormat::IeeeF32 => {
                        let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                        if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
                    }
                    WavFormat::IeeeF64 => {
                        let v = f64::from_le_bytes([
                            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
                        ]);
                        if v.is_finite() {
                            (v as f32).clamp(-1.0, 1.0)
                        } else {
                            0.0
                        }
                    }
                    WavFormat::Alaw => decode_alaw_sample(s[0]),
                    WavFormat::Mulaw => decode_mulaw_sample(s[0]),
                    _ => unreachable!(),
                }
            };
            for frame in raw.chunks_exact(frame_bytes) {
                let l;
                let r;
                if ch == 1 {
                    let s = read_one(&frame[0..bps]);
                    l = s;
                    r = s;
                } else if ch == 2 {
                    l = read_one(&frame[0..bps]);
                    r = read_one(&frame[bps..bps * 2]);
                } else {
                    let mut sum_l = 0.0f32;
                    let mut sum_r = 0.0f32;
                    let mut cnt_l = 0.0f32;
                    let mut cnt_r = 0.0f32;
                    for c in 0..ch {
                        let s = read_one(&frame[c * bps..(c + 1) * bps]);
                        if (c & 1) == 0 {
                            sum_l += s;
                            cnt_l += 1.0;
                        } else {
                            sum_r += s;
                            cnt_r += 1.0;
                        }
                    }
                    l = if cnt_l > 0.0 { sum_l / cnt_l } else { 0.0 };
                    r = if cnt_r > 0.0 { sum_r / cnt_r } else { l };
                }
                out.push(l);
                out.push(r);
            }
            Ok(out)
        }
        WavFormat::ImaAdpcm {
            block_align,
            samples_per_block,
        } => {
            let ba = block_align as usize;
            let num_blocks = raw.len() / ba;
            let est_frames = num_blocks.saturating_mul((samples_per_block as usize).max(1));
            let mut out = Vec::with_capacity(est_frames.min(16_777_216) * 2);

            for blk in raw.chunks_exact(ba) {
                if ch == 1 {
                    let mut pred = i16::from_le_bytes([blk[0], blk[1]]) as i32;
                    let mut step_idx = (blk[2] as i32).clamp(0, 88);
                    let first = (pred as f32) / 32768.0;
                    out.push(first);
                    out.push(first);
                    let mut emitted = 1usize;
                    let max_blk = samples_per_block as usize;
                    for &byte in &blk[4..] {
                        if emitted >= max_blk {
                            break;
                        }
                        let s0 = ima_step(byte & 0x0F, &mut pred, &mut step_idx);
                        out.push(s0);
                        out.push(s0);
                        emitted += 1;
                        if emitted >= max_blk {
                            break;
                        }
                        let s1 = ima_step((byte >> 4) & 0x0F, &mut pred, &mut step_idx);
                        out.push(s1);
                        out.push(s1);
                        emitted += 1;
                    }
                } else {
                    let mut pred_l = i16::from_le_bytes([blk[0], blk[1]]) as i32;
                    let mut idx_l = (blk[2] as i32).clamp(0, 88);
                    let mut pred_r = i16::from_le_bytes([blk[4], blk[5]]) as i32;
                    let mut idx_r = (blk[6] as i32).clamp(0, 88);
                    out.push((pred_l as f32) / 32768.0);
                    out.push((pred_r as f32) / 32768.0);
                    let mut emitted = 1usize;
                    let max_blk = samples_per_block as usize;
                    let payload = &blk[8..];
                    for pair4 in payload.chunks_exact(8) {
                        let mut ls = [0.0f32; 8];
                        let mut rs = [0.0f32; 8];
                        for i in 0..4 {
                            let bl = pair4[i];
                            ls[i * 2] = ima_step(bl & 0x0F, &mut pred_l, &mut idx_l);
                            ls[i * 2 + 1] = ima_step((bl >> 4) & 0x0F, &mut pred_l, &mut idx_l);
                            let br = pair4[4 + i];
                            rs[i * 2] = ima_step(br & 0x0F, &mut pred_r, &mut idx_r);
                            rs[i * 2 + 1] = ima_step((br >> 4) & 0x0F, &mut pred_r, &mut idx_r);
                        }
                        for i in 0..8 {
                            if emitted >= max_blk {
                                break;
                            }
                            out.push(ls[i]);
                            out.push(rs[i]);
                            emitted += 1;
                        }
                    }
                }
            }
            Ok(out)
        }
    }
}

pub fn resample_stereo_linear(
    src: &[f32],
    src_rate: u32,
    dst_rate: u32,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<Vec<f32>, String> {
    let src_frames = src.len() / 2;
    if src_frames == 0 || src_rate == 0 || dst_rate == 0 {
        return Ok(Vec::new());
    }
    if src_rate == dst_rate {
        return Ok(src.to_vec());
    }
    let dst_frames_u64 = ((src_frames as u64).saturating_mul(dst_rate as u64)).div_ceil(src_rate as u64);
    let dst_bytes = (dst_frames_u64 as usize).saturating_mul(8);
    if dst_bytes > MAX_SOUND_BYTES {
        return Err(err_limit(format!(
            "resampled audio {} bytes exceeds {} byte limit",
            dst_bytes, MAX_SOUND_BYTES
        )));
    }
    let dst_frames = dst_frames_u64 as usize;
    let mut out = Vec::with_capacity(dst_frames * 2);
    // Fixed-point 32.32 phase increment
    let step_q32: u64 = ((src_rate as u64) << 32) / (dst_rate as u64);
    let mut pos_q32: u64 = 0;

    for idx in 0..dst_frames {
        if (idx & 4095) == 0 {
            check_deadline(deadline)?;
        }
        let i0 = ((pos_q32 >> 32) as usize).min(src_frames - 1);
        let i1 = (i0 + 1).min(src_frames - 1);
        let frac = ((pos_q32 & 0xFFFF_FFFF) as f32) * (1.0 / 4_294_967_296.0);
        let l0 = src[i0 * 2];
        let r0 = src[i0 * 2 + 1];
        let l1 = src[i1 * 2];
        let r1 = src[i1 * 2 + 1];
        out.push(l0 + (l1 - l0) * frac);
        out.push(r0 + (r1 - r0) * frac);
        pos_q32 = pos_q32.saturating_add(step_q32);
    }
    Ok(out)
}

pub fn decode_wav_sound(
    data: &[u8],
    loop_override: Option<(u64, u64)>,
    budget_core: Option<&Arc<BudgetCore>>,
    deadline: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<SoundData, String> {
    check_deadline(deadline)?;
    let meta = parse_wav_header(data)?;
    let raw_slice = &data[meta.data_offset..meta.data_offset + meta.data_len];

    // Estimate decoded byte size strictly before allocating buffers.
    let raw_frames = match meta.format {
        WavFormat::PcmU8 | WavFormat::Alaw | WavFormat::Mulaw => meta.data_len / (meta.channels as usize),
        WavFormat::PcmS16 => meta.data_len / ((meta.channels as usize) * 2),
        WavFormat::PcmS24 => meta.data_len / ((meta.channels as usize) * 3),
        WavFormat::PcmS32 | WavFormat::IeeeF32 => meta.data_len / ((meta.channels as usize) * 4),
        WavFormat::IeeeF64 => meta.data_len / ((meta.channels as usize) * 8),
        WavFormat::ImaAdpcm {
            block_align,
            samples_per_block,
        } => (meta.data_len / (block_align as usize)).saturating_mul(samples_per_block as usize),
    };
    let est_out_frames = if meta.sample_rate == ENGINE_RATE {
        raw_frames as u64
    } else {
        ((raw_frames as u64).saturating_mul(ENGINE_RATE as u64)).div_ceil(meta.sample_rate as u64)
    };
    let est_bytes = (est_out_frames as usize).saturating_mul(8);
    if est_bytes > MAX_SOUND_BYTES {
        return Err(err_limit(format!(
            "decoded sound {} bytes exceeds {} byte limit",
            est_bytes, MAX_SOUND_BYTES
        )));
    }
    let _tmp_raw = temp_budget_guard(budget_core, raw_frames.saturating_mul(8).min(MAX_SOUND_BYTES))?;
    let stereo_src = decode_wav_chunk_to_stereo(&meta, raw_slice)?;
    check_deadline(deadline)?;
    let resampled = resample_stereo_linear(&stereo_src, meta.sample_rate, ENGINE_RATE, deadline)?;
    drop(stereo_src);
    drop(_tmp_raw);

    let resampled_frames = (resampled.len() / 2) as u64;
    let mapped_loop = loop_override.or_else(|| {
        meta.loop_points.map(|(a, b)| {
            let sa = (a.saturating_mul(ENGINE_RATE as u64)) / (meta.sample_rate as u64);
            let sb = ((b.saturating_mul(ENGINE_RATE as u64)) / (meta.sample_rate as u64)).min(resampled_frames);
            (sa.min(sb), sb)
        })
    });

    if let Some((a, b)) = mapped_loop {
        if a >= b || b > resampled_frames {
            return Err(err_bad_opt("audio loop points out of bounds"));
        }
    }

    let digest = Digest::from_parts(&[b"askane.wav.v1|", data]);
    SoundData::from_pcm(resampled, mapped_loop, Some(digest), budget_core)
}

/// Validates and constructs an incremental WAV stream decoder returning ENGINE_RATE stereo f32 chunks.
pub struct WavStreamDecoder {
    meta: WavMeta,
    bytes: Arc<Vec<u8>>,
    byte_cursor: usize,
    _budget: crate::media::MediaBudgetGuard,
    pending_src: Vec<f32>,
    pending_cursor_q32: u64,
    step_q32: u64,
}

impl WavStreamDecoder {
    pub fn from_bytes(
        bytes: Vec<u8>,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        let meta = match parse_wav_header(&bytes) {
            Ok(m) => m,
            Err(e) => {
                if bytes.len() >= 4
                    && (&bytes[0..4] == b"OggS" || &bytes[0..4] == b"fLaC" || &bytes[0..3] == b"ID3")
                {
                    return Err(err_stream_unsupported(
                        "streaming compressed OGG/FLAC/MP3 is not supported in 0.20 (use WAV/ADPCM/PCM)",
                    ));
                }
                return Err(e);
            }
        };
        let guard = crate::media::MediaBudgetGuard::charge(budget_core, bytes.len())?;
        let offset = meta.data_offset;
        let sr = meta.sample_rate;
        Ok(Self {
            meta,
            bytes: Arc::new(bytes),
            byte_cursor: offset,
            _budget: guard,
            pending_src: Vec::new(),
            pending_cursor_q32: 0,
            step_q32: ((sr as u64) << 32) / (ENGINE_RATE as u64),
        })
    }

    pub fn estimated_total_engine_frames(&self) -> u64 {
        let ch = self.meta.channels as usize;
        let raw_frames = match self.meta.format {
            WavFormat::PcmU8 | WavFormat::Alaw | WavFormat::Mulaw => self.meta.data_len / ch,
            WavFormat::PcmS16 => self.meta.data_len / (ch * 2),
            WavFormat::PcmS24 => self.meta.data_len / (ch * 3),
            WavFormat::PcmS32 | WavFormat::IeeeF32 => self.meta.data_len / (ch * 4),
            WavFormat::IeeeF64 => self.meta.data_len / (ch * 8),
            WavFormat::ImaAdpcm {
                block_align,
                samples_per_block,
            } => (self.meta.data_len / (block_align as usize)).saturating_mul(samples_per_block as usize),
        };
        ((raw_frames as u64).saturating_mul(ENGINE_RATE as u64)).div_ceil(self.meta.sample_rate as u64)
    }

    pub fn seek_engine_frame(&mut self, frame: u64) {
        let src_frame = (frame.saturating_mul(self.meta.sample_rate as u64)) / (ENGINE_RATE as u64);
        let ch = self.meta.channels as usize;
        let byte_off = match self.meta.format {
            WavFormat::PcmU8 | WavFormat::Alaw | WavFormat::Mulaw => (src_frame as usize) * ch,
            WavFormat::PcmS16 => (src_frame as usize) * ch * 2,
            WavFormat::PcmS24 => (src_frame as usize) * ch * 3,
            WavFormat::PcmS32 | WavFormat::IeeeF32 => (src_frame as usize) * ch * 4,
            WavFormat::IeeeF64 => (src_frame as usize) * ch * 8,
            WavFormat::ImaAdpcm {
                block_align,
                samples_per_block,
            } => {
                let spb = (samples_per_block as usize).max(1);
                let blk_idx = (src_frame as usize) / spb;
                blk_idx.saturating_mul(block_align as usize)
            }
        };
        let end = self.meta.data_offset.saturating_add(self.meta.data_len);
        self.byte_cursor = self.meta.data_offset.saturating_add(byte_off).min(end);
        self.pending_src.clear();
        self.pending_cursor_q32 = 0;
    }

    /// Reads up to `max_out_frames` resampled stereo frames; returns `(samples, reached_eof)`.
    pub fn read_next_stereo(&mut self, max_out_frames: usize) -> Result<(Vec<f32>, bool), String> {
        let data_end = self.meta.data_offset + self.meta.data_len;
        let mut out = Vec::with_capacity(max_out_frames * 2);

        while out.len() / 2 < max_out_frames {
            let pending_frames = self.pending_src.len() / 2;
            let needed_idx = (self.pending_cursor_q32 >> 32) as usize;
            if needed_idx + 1 >= pending_frames {
                // Keep last sample for linear overlap if available
                let leftover = if pending_frames > 0 && needed_idx < pending_frames {
                    vec![
                        self.pending_src[(pending_frames - 1) * 2],
                        self.pending_src[(pending_frames - 1) * 2 + 1],
                    ]
                } else {
                    Vec::new()
                };
                self.pending_cursor_q32 &= 0xFFFF_FFFF;

                if self.byte_cursor >= data_end {
                    self.pending_src = leftover;
                    break;
                }

                let unit = match self.meta.format {
                    WavFormat::PcmU8 | WavFormat::Alaw | WavFormat::Mulaw => self.meta.channels as usize,
                    WavFormat::PcmS16 => (self.meta.channels as usize) * 2,
                    WavFormat::PcmS24 => (self.meta.channels as usize) * 3,
                    WavFormat::PcmS32 | WavFormat::IeeeF32 => (self.meta.channels as usize) * 4,
                    WavFormat::IeeeF64 => (self.meta.channels as usize) * 8,
                    WavFormat::ImaAdpcm { block_align, .. } => block_align as usize,
                };
                let target_units = (2048usize / unit.max(1)).max(1) * unit;
                let avail = data_end - self.byte_cursor;
                let take = avail.min(target_units);
                let aligned_take = (take / unit) * unit;
                if aligned_take == 0 {
                    self.byte_cursor = data_end;
                    self.pending_src = leftover;
                    break;
                }
                let decoded = decode_wav_chunk_to_stereo(
                    &self.meta,
                    &self.bytes[self.byte_cursor..self.byte_cursor + aligned_take],
                )?;
                self.byte_cursor += aligned_take;
                let had_leftover = !leftover.is_empty();
                self.pending_src = leftover;
                self.pending_src.extend_from_slice(&decoded);
                if !had_leftover {
                    self.pending_cursor_q32 = 0;
                }
            }

            let cur_frames = self.pending_src.len() / 2;
            if cur_frames == 0 {
                break;
            }
            let i0 = ((self.pending_cursor_q32 >> 32) as usize).min(cur_frames - 1);
            let i1 = (i0 + 1).min(cur_frames - 1);
            let frac = ((self.pending_cursor_q32 & 0xFFFF_FFFF) as f32) * (1.0 / 4_294_967_296.0);
            let l0 = self.pending_src[i0 * 2];
            let r0 = self.pending_src[i0 * 2 + 1];
            let l1 = self.pending_src[i1 * 2];
            let r1 = self.pending_src[i1 * 2 + 1];
            out.push(l0 + (l1 - l0) * frac);
            out.push(r0 + (r1 - r0) * frac);
            self.pending_cursor_q32 = self.pending_cursor_q32.saturating_add(self.step_q32);
        }

        let eof = self.byte_cursor >= data_end
            && ((self.pending_cursor_q32 >> 32) as usize + 1 >= self.pending_src.len() / 2);
        Ok((out, eof))
    }
}

/// Deterministic 16-bit PCM Stereo WAV encoder for `audio.render_offline()`.
pub fn encode_wav_s16le(stereo_f32: &[f32], sample_rate: u32) -> Vec<u8> {
    let frames = stereo_f32.len() / 2;
    let data_bytes = (frames * 4) as u32;
    let riff_size = 36u32.saturating_add(data_bytes);
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // Stereo
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate.saturating_mul(4)).to_le_bytes()); // byte rate
    out.extend_from_slice(&4u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in &stereo_f32[..frames * 2] {
        let clamped = if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };
        let scaled = if clamped >= 0.0 {
            (clamped * 32767.0 + 0.5) as i32
        } else {
            (clamped * 32768.0 - 0.5) as i32
        };
        let i = scaled.clamp(-32768, 32767) as i16;
        out.extend_from_slice(&i.to_le_bytes());
    }
    out
}
