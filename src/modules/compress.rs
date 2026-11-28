use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub const DEFAULT_MAX_DECOMPRESS: usize = 32 * 1024 * 1024;

pub fn module() -> Value {
    build_module(
        "compress",
        &[
            ("deflate", mod_deflate),
            ("inflate", mod_inflate),
            ("zlib_compress", mod_zlib_compress),
            ("zlib_decompress", mod_zlib_decompress),
            ("gzip_compress", mod_gzip_compress),
            ("gzip_decompress", mod_gzip_decompress),
            ("crc32", mod_crc32),
            ("adler32", mod_adler32),
        ],
        &[],
    )
}

pub(crate) fn crc32_bytes(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & mask);
        }
    }
    !crc
}

pub(crate) fn adler32_bytes(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_buf: u64,
    bit_cnt: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_buf: 0,
            bit_cnt: 0,
        }
    }

    fn ensure(&mut self, n: u8) -> Result<(), String> {
        while self.bit_cnt < n {
            if self.byte_pos >= self.data.len() {
                return Err("Unexpected end of compressed deflate stream".to_string());
            }
            self.bit_buf |= (self.data[self.byte_pos] as u64) << self.bit_cnt;
            self.byte_pos += 1;
            self.bit_cnt += 8;
        }
        Ok(())
    }

    #[inline]
    fn read_bits(&mut self, n: u8) -> Result<u32, String> {
        if n == 0 {
            return Ok(0);
        }
        self.ensure(n)?;
        let val = (self.bit_buf & ((1u64 << n) - 1)) as u32;
        self.bit_buf >>= n;
        self.bit_cnt -= n;
        Ok(val)
    }

    fn align_byte(&mut self) {
        let discard = self.bit_cnt & 7;
        self.bit_buf >>= discard;
        self.bit_cnt -= discard;
    }

    fn read_aligned_bytes(&mut self, len: usize, out: &mut Vec<u8>) -> Result<(), String> {
        let buffered = (self.bit_cnt / 8) as usize;
        if buffered > self.byte_pos {
            return Err("Corrupt bit stream alignment".to_string());
        }
        self.byte_pos -= buffered;
        self.bit_buf = 0;
        self.bit_cnt = 0;
        if self.byte_pos + len > self.data.len() {
            return Err("Unexpected EOF inside stored deflate block".to_string());
        }
        out.extend_from_slice(&self.data[self.byte_pos..self.byte_pos + len]);
        self.byte_pos += len;
        Ok(())
    }

    fn read_u16_aligned(&mut self) -> Result<u16, String> {
        self.align_byte();
        let lo = self.read_bits(8)? as u16;
        let hi = self.read_bits(8)? as u16;
        Ok(lo | (hi << 8))
    }
}

struct HuffTree {
    counts: [u16; 16],
    syms: Vec<u16>,
}

impl HuffTree {
    fn from_lengths(lengths: &[u8]) -> Result<Self, String> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l as usize > 15 {
                return Err("Huffman code length > 15".to_string());
            }
            if l > 0 {
                counts[l as usize] += 1;
            }
        }
        let mut offsets = [0u16; 16];
        let mut total = 0u16;
        for bits in 1..=15 {
            offsets[bits] = total;
            total += counts[bits];
        }
        let mut left = 1i32;
        for bits in 1..=15 {
            left <<= 1;
            left -= counts[bits] as i32;
            if left < 0 {
                return Err("Oversubscribed Huffman tree".to_string());
            }
        }
        let mut syms = vec![0u16; total as usize];
        for (sym, &l) in lengths.iter().enumerate() {
            if l > 0 {
                let pos = offsets[l as usize] as usize;
                offsets[l as usize] += 1;
                syms[pos] = sym as u16;
            }
        }
        Ok(Self { counts, syms })
    }

    #[inline]
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, String> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=15 {
            code |= reader.read_bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return self
                    .syms
                    .get((index + (code - first)) as usize)
                    .copied()
                    .ok_or_else(|| "Invalid Huffman symbol".to_string());
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("Invalid Huffman code in deflate stream".to_string())
    }
}

fn fixed_trees() -> Result<(HuffTree, HuffTree), String> {
    let mut lit_lens = [0u8; 288];
    lit_lens[..=143].fill(8);
    lit_lens[144..=255].fill(9);
    lit_lens[256..=279].fill(7);
    lit_lens[280..=287].fill(8);
    let dist_lens = [5u8; 32];
    Ok((
        HuffTree::from_lengths(&lit_lens)?,
        HuffTree::from_lengths(&dist_lens)?,
    ))
}

fn dynamic_trees(reader: &mut BitReader<'_>) -> Result<(HuffTree, HuffTree), String> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let hlit = reader.read_bits(5)? as usize + 257;
    let hdist = reader.read_bits(5)? as usize + 1;
    let hclen = reader.read_bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err("Invalid dynamic Huffman table header".to_string());
    }
    let mut cl_lens = [0u8; 19];
    for idx in 0..hclen {
        cl_lens[ORDER[idx]] = reader.read_bits(3)? as u8;
    }
    let cl_tree = HuffTree::from_lengths(&cl_lens)?;
    let total = hlit + hdist;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = cl_tree.decode(reader)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let prev = *lengths
                    .last()
                    .ok_or_else(|| "Invalid code length repeat (16)".to_string())?;
                let rep = reader.read_bits(2)? as usize + 3;
                lengths.resize((lengths.len() + rep).min(total), prev);
            }
            17 => {
                let rep = reader.read_bits(3)? as usize + 3;
                lengths.resize((lengths.len() + rep).min(total), 0);
            }
            18 => {
                let rep = reader.read_bits(7)? as usize + 11;
                lengths.resize((lengths.len() + rep).min(total), 0);
            }
            _ => return Err("Invalid code length symbol".to_string()),
        }
    }
    Ok((
        HuffTree::from_lengths(&lengths[..hlit])?,
        HuffTree::from_lengths(&lengths[hlit..hlit + hdist])?,
    ))
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

pub(crate) fn inflate_raw(
    input: &[u8],
    max_output: usize,
    mut vm: Option<&mut VM>,
) -> Result<Vec<u8>, String> {
    let mut reader = BitReader::new(input);
    let mut out = Vec::with_capacity(input.len().min(max_output).min(64 * 1024));
    let mut steps = 0usize;

    loop {
        steps += 1;
        if (steps & 255) == 0 {
            if let Some(ref mut v) = vm {
                v.check_timeout()?;
                v.charge_work(64)?;
            }
        }
        let bfinal = reader.read_bits(1)?;
        let btype = reader.read_bits(2)?;
        match btype {
            0 => {
                let len = reader.read_u16_aligned()? as usize;
                let nlen = reader.read_u16_aligned()? as usize;
                if (len ^ 0xFFFF) != nlen {
                    return Err("Stored deflate block length checksum mismatch".to_string());
                }
                if out.len().saturating_add(len) > max_output {
                    return Err(format!(
                        "Decompressed output exceeds safety limit ({} bytes, possible zip bomb)",
                        max_output
                    ));
                }
                if let Some(ref mut v) = vm {
                    v.charge_alloc(len)?;
                    v.charge_work((len / 64).max(1) as u64)?;
                }
                reader.read_aligned_bytes(len, &mut out)?;
            }
            1 | 2 => {
                let (lit_tree, dist_tree) = if btype == 1 {
                    fixed_trees()?
                } else {
                    dynamic_trees(&mut reader)?
                };
                loop {
                    steps += 1;
                    if (steps & 4095) == 0 {
                        if let Some(ref mut v) = vm {
                            v.check_timeout()?;
                            v.charge_work(256)?;
                        }
                    }
                    let sym = lit_tree.decode(&mut reader)?;
                    if sym < 256 {
                        if out.len() >= max_output {
                            return Err(format!(
                                "Decompressed output exceeds safety limit ({} bytes)",
                                max_output
                            ));
                        }
                        out.push(sym as u8);
                        if let Some(ref mut v) = vm {
                            if (out.len() & 4095) == 0 {
                                v.charge_alloc(4096)?;
                            }
                        }
                    } else if sym == 256 {
                        break;
                    } else {
                        let l_idx = (sym - 257) as usize;
                        if l_idx >= LEN_BASE.len() {
                            return Err("Invalid deflate length code".to_string());
                        }
                        let length =
                            LEN_BASE[l_idx] as usize + reader.read_bits(LEN_EXTRA[l_idx])? as usize;
                        let d_sym = dist_tree.decode(&mut reader)? as usize;
                        if d_sym >= DIST_BASE.len() {
                            return Err("Invalid deflate distance code".to_string());
                        }
                        let dist = DIST_BASE[d_sym] as usize
                            + reader.read_bits(DIST_EXTRA[d_sym])? as usize;
                        if dist == 0 || dist > out.len() {
                            return Err("Invalid deflate back-reference distance".to_string());
                        }
                        if out.len().saturating_add(length) > max_output {
                            return Err(format!(
                                "Decompressed output exceeds safety limit ({} bytes, possible zip bomb)",
                                max_output
                            ));
                        }
                        if let Some(ref mut v) = vm {
                            v.charge_alloc(length)?;
                            v.charge_work((length / 64).max(1) as u64)?;
                        }
                        for _ in 0..length {
                            let b = out[out.len() - dist];
                            out.push(b);
                        }
                    }
                }
            }
            _ => return Err("Invalid deflate block type 3".to_string()),
        }
        if bfinal != 0 {
            break;
        }
    }
    Ok(out)
}

pub(crate) fn inflate_zlib(
    input: &[u8],
    max_output: usize,
    vm: Option<&mut VM>,
) -> Result<Vec<u8>, String> {
    if input.len() < 6 {
        return Err("zlib stream too short".to_string());
    }
    let cmf = input[0];
    let flg = input[1];
    if (cmf & 0x0F) != 8 {
        return Err("Unsupported zlib compression method (expected deflate)".to_string());
    }
    if (((cmf as u16) << 8) | flg as u16) % 31 != 0 {
        return Err("Invalid zlib header check bits".to_string());
    }
    if (flg & 0x20) != 0 {
        return Err("Unsupported zlib preset dictionary (FDICT)".to_string());
    }
    let data_end = input.len() - 4;
    let out = inflate_raw(&input[2..data_end], max_output, vm)?;
    let expected = u32::from_be_bytes(input[data_end..].try_into().unwrap());
    let actual = adler32_bytes(&out);
    if expected != actual {
        return Err("zlib Adler-32 checksum mismatch".to_string());
    }
    Ok(out)
}

struct BitWriter {
    out: Vec<u8>,
    bit_buf: u64,
    bit_cnt: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            bit_buf: 0,
            bit_cnt: 0,
        }
    }

    #[inline]
    fn write_bits_lsb(&mut self, val: u32, n: u8) {
        self.bit_buf |= (val as u64) << self.bit_cnt;
        self.bit_cnt += n;
        while self.bit_cnt >= 8 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_cnt -= 8;
        }
    }

    #[inline]
    fn write_huff_msb(&mut self, code: u16, len: u8) {
        for bit in (0..len).rev() {
            let b = ((code >> bit) & 1) as u32;
            self.write_bits_lsb(b, 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bit_cnt > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
        }
        self.out
    }
}

fn emit_fixed_lit(w: &mut BitWriter, sym: u16) {
    match sym {
        0..=143 => w.write_huff_msb(0x30 + sym, 8),
        144..=255 => w.write_huff_msb(0x190 + (sym - 144), 9),
        256..=279 => w.write_huff_msb(sym - 256, 7),
        280..=287 => w.write_huff_msb(0xC0 + (sym - 280), 8),
        _ => unreachable!(),
    }
}

fn encode_length_dist(w: &mut BitWriter, length: usize, dist: usize) {
    let mut l_idx = 0usize;
    while l_idx + 1 < LEN_BASE.len() && LEN_BASE[l_idx + 1] as usize <= length {
        l_idx += 1;
    }
    emit_fixed_lit(w, 257 + l_idx as u16);
    if LEN_EXTRA[l_idx] > 0 {
        w.write_bits_lsb((length - LEN_BASE[l_idx] as usize) as u32, LEN_EXTRA[l_idx]);
    }
    let mut d_idx = 0usize;
    while d_idx + 1 < DIST_BASE.len() && DIST_BASE[d_idx + 1] as usize <= dist {
        d_idx += 1;
    }
    w.write_huff_msb(d_idx as u16, 5);
    if DIST_EXTRA[d_idx] > 0 {
        w.write_bits_lsb((dist - DIST_BASE[d_idx] as usize) as u32, DIST_EXTRA[d_idx]);
    }
}

pub(crate) fn deflate_raw(input: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits_lsb(1, 1); // BFINAL
    w.write_bits_lsb(1, 2); // BTYPE = fixed Huffman

    let mut head = vec![u32::MAX; 4096];
    let mut i = 0usize;
    while i < input.len() {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        if i + 3 <= input.len() {
            let h = (((input[i] as usize) << 8)
                ^ ((input[i + 1] as usize) << 4)
                ^ (input[i + 2] as usize))
                & 4095;
            let prev = head[h];
            head[h] = i as u32;
            if prev != u32::MAX {
                let p = prev as usize;
                let dist = i - p;
                if dist > 0 && dist <= 32768 {
                    let max_l = (input.len() - i).min(258);
                    let mut m = 0usize;
                    while m < max_l && input[p + m] == input[i + m] {
                        m += 1;
                    }
                    if m >= 3 {
                        best_len = m;
                        best_dist = dist;
                    }
                }
            }
        }
        if best_len >= 3 {
            encode_length_dist(&mut w, best_len, best_dist);
            for k in (i + 1)..(i + best_len).min(input.len().saturating_sub(2)) {
                let h = (((input[k] as usize) << 8)
                    ^ ((input[k + 1] as usize) << 4)
                    ^ (input[k + 2] as usize))
                    & 4095;
                head[h] = k as u32;
            }
            i += best_len;
        } else {
            emit_fixed_lit(&mut w, input[i] as u16);
            i += 1;
        }
    }
    emit_fixed_lit(&mut w, 256);
    w.finish()
}

fn max_out_arg(vm: &VM, args: &[Value], idx: usize) -> Result<usize, String> {
    let cap = if args.len() > idx {
        let Value::Int(m) = args[idx] else {
            return Err("max_size must be positive integer".to_string());
        };
        if m <= 0 {
            return Err("max_size must be > 0".to_string());
        }
        m as usize
    } else {
        DEFAULT_MAX_DECOMPRESS
    };
    let budget_cap = vm
        .budget_snapshot()
        .1
        .map(|l| l as usize)
        .unwrap_or(usize::MAX);
    Ok(cap.min(budget_cap).min(128 * 1024 * 1024))
}

fn mod_deflate(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("compress.deflate() expects 1 argument (bytes/str)".to_string());
    }
    let raw = args[0].as_bytes_cow("compress.deflate")?;
    vm.charge_work((raw.len() as u64) / 8 + 1)?;
    let out = deflate_raw(&raw);
    vm.charge_alloc(out.len())?;
    Ok(Value::Bytes(Rc::new(out)))
}

fn mod_inflate(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("compress.inflate() expects (bytes, max_size?)".to_string());
    }
    let max_out = max_out_arg(vm, args, 1)?;
    let raw = args[0].as_bytes_cow("compress.inflate")?;
    let out = inflate_raw(&raw, max_out, Some(vm))?;
    Ok(Value::Bytes(Rc::new(out)))
}

fn mod_zlib_compress(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("compress.zlib_compress() expects 1 argument".to_string());
    }
    let raw = args[0].as_bytes_cow("compress.zlib_compress")?;
    vm.charge_work((raw.len() as u64) / 8 + 1)?;
    let body = deflate_raw(&raw);
    let adler = adler32_bytes(&raw);
    let mut out = Vec::with_capacity(body.len() + 6);
    out.push(0x78);
    out.push(0x9C);
    out.extend_from_slice(&body);
    out.extend_from_slice(&adler.to_be_bytes());
    vm.charge_alloc(out.len())?;
    Ok(Value::Bytes(Rc::new(out)))
}

fn mod_zlib_decompress(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("compress.zlib_decompress() expects (bytes, max_size?)".to_string());
    }
    let max_out = max_out_arg(vm, args, 1)?;
    let raw = args[0].as_bytes_cow("compress.zlib_decompress")?;
    let out = inflate_zlib(&raw, max_out, Some(vm))?;
    Ok(Value::Bytes(Rc::new(out)))
}

fn mod_gzip_compress(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("compress.gzip_compress() expects 1 argument".to_string());
    }
    let raw = args[0].as_bytes_cow("compress.gzip_compress")?;
    vm.charge_work((raw.len() as u64) / 8 + 1)?;
    let body = deflate_raw(&raw);
    let crc = crc32_bytes(&raw);
    let isize_val = (raw.len() as u32).to_le_bytes();
    let mut out = Vec::with_capacity(body.len() + 18);
    out.extend_from_slice(&[0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 255]);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&isize_val);
    vm.charge_alloc(out.len())?;
    Ok(Value::Bytes(Rc::new(out)))
}

pub(crate) fn inflate_gzip(
    raw: &[u8],
    max_output: usize,
    vm: Option<&mut VM>,
) -> Result<Vec<u8>, String> {
    if raw.len() < 18 || raw[0] != 0x1F || raw[1] != 0x8B || raw[2] != 8 {
        return Err("Invalid gzip header".to_string());
    }
    let flg = raw[3];
    let mut pos = 10usize;
    if (flg & 0x04) != 0 {
        if pos + 2 > raw.len() {
            return Err("Truncated gzip extra header".to_string());
        }
        let xlen = u16::from_le_bytes([raw[pos], raw[pos + 1]]) as usize;
        pos = pos
            .checked_add(2 + xlen)
            .ok_or_else(|| "Corrupt gzip extra header".to_string())?;
    }
    for flag in [0x08u8, 0x10u8] {
        if (flg & flag) != 0 {
            while pos < raw.len() && raw[pos] != 0 {
                pos += 1;
            }
            if pos >= raw.len() {
                return Err("Truncated gzip string header".to_string());
            }
            pos += 1;
        }
    }
    if (flg & 0x02) != 0 {
        pos = pos
            .checked_add(2)
            .ok_or_else(|| "Truncated gzip CRC16 header".to_string())?;
    }
    if pos + 8 > raw.len() {
        return Err("Truncated gzip stream".to_string());
    }
    let out = inflate_raw(&raw[pos..raw.len() - 8], max_output, vm)?;
    let expected_crc = u32::from_le_bytes(raw[raw.len() - 8..raw.len() - 4].try_into().unwrap());
    let expected_len = u32::from_le_bytes(raw[raw.len() - 4..].try_into().unwrap());
    if crc32_bytes(&out) != expected_crc || (out.len() as u32) != expected_len {
        return Err("gzip checksum or length mismatch".to_string());
    }
    Ok(out)
}

fn mod_gzip_decompress(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("compress.gzip_decompress() expects (bytes, max_size?)".to_string());
    }
    let max_out = max_out_arg(vm, args, 1)?;
    let raw = args[0].as_bytes_cow("compress.gzip_decompress")?;
    let out = inflate_gzip(&raw, max_out, Some(vm))?;
    Ok(Value::Bytes(Rc::new(out)))
}

fn mod_crc32(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("compress.crc32() expects 1 argument".to_string());
    }
    let raw = args[0].as_bytes_cow("compress.crc32")?;
    vm.charge_work((raw.len() as u64) / 32 + 1)?;
    Ok(Value::Int(crc32_bytes(&raw) as i64))
}

fn mod_adler32(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("compress.adler32() expects 1 argument".to_string());
    }
    let raw = args[0].as_bytes_cow("compress.adler32")?;
    vm.charge_work((raw.len() as u64) / 32 + 1)?;
    Ok(Value::Int(adler32_bytes(&raw) as i64))
}
