use std::rc::Rc;

use crate::modules::build_module;
use crate::modules::hash::sha256_bytes;
use crate::modules::random::os_random_bytes;
use crate::value::Value;
use crate::vm::VM;

const MAX_CRYPTO_INPUT: usize = 32 * 1024 * 1024;

pub fn module() -> Value {
    build_module(
        "crypto",
        &[
            ("hmac_sha256", crypto_hmac_sha256),
            ("hmac_sha256_hex", crypto_hmac_sha256_hex),
            ("constant_time_eq", crypto_constant_time_eq),
            ("random_key", crypto_random_key),
            ("random_bytes", crypto_random_bytes),
            ("seal", crypto_seal),
            ("open", crypto_open),
        ],
        &[],
    )
}

pub(crate) fn hmac_sha256_raw(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = sha256_bytes(key);
        k_block[..32].copy_from_slice(&hashed);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }

    let mut inner = Vec::with_capacity(64 + data.len());
    for b in k_block {
        inner.push(b ^ 0x36);
    }
    inner.extend_from_slice(data);
    let inner_hash = sha256_bytes(&inner);

    let mut outer = Vec::with_capacity(64 + 32);
    for b in k_block {
        outer.push(b ^ 0x5c);
    }
    outer.extend_from_slice(&inner_hash);
    sha256_bytes(&outer)
}

pub(crate) fn ct_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[inline]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut initial = [0u32; 16];
    initial[0] = 0x6170_7865;
    initial[1] = 0x3320_646e;
    initial[2] = 0x7962_2d32;
    initial[3] = 0x6b20_6574;
    for i in 0..8 {
        initial[4 + i] =
            u32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
    }
    initial[12] = counter;
    for i in 0..3 {
        initial[13 + i] = u32::from_le_bytes([
            nonce[i * 4],
            nonce[i * 4 + 1],
            nonce[i * 4 + 2],
            nonce[i * 4 + 3],
        ]);
    }

    let mut working = initial;
    for _ in 0..10 {
        quarter_round(&mut working, 0, 4, 8, 12);
        quarter_round(&mut working, 1, 5, 9, 13);
        quarter_round(&mut working, 2, 6, 10, 14);
        quarter_round(&mut working, 3, 7, 11, 15);
        quarter_round(&mut working, 0, 5, 10, 15);
        quarter_round(&mut working, 1, 6, 11, 12);
        quarter_round(&mut working, 2, 7, 8, 13);
        quarter_round(&mut working, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let w = working[i].wrapping_add(initial[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn chacha20_xor(key: &[u8; 32], start_counter: u32, nonce: &[u8; 12], input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut counter = start_counter;
    for chunk in input.chunks(64) {
        let block = chacha20_block(key, counter, nonce);
        for (i, &b) in chunk.iter().enumerate() {
            out.push(b ^ block[i]);
        }
        counter = counter.wrapping_add(1);
    }
    out
}

struct Poly1305 {
    r: [u32; 5],
    s: [u32; 4],
    h: [u32; 5],
    pad: [u32; 4],
}

impl Poly1305 {
    fn new(key: &[u8; 32]) -> Self {
        let mut rk = [0u8; 16];
        rk.copy_from_slice(&key[0..16]);
        rk[3] &= 15;
        rk[7] &= 15;
        rk[11] &= 15;
        rk[15] &= 15;
        rk[4] &= 252;
        rk[8] &= 252;
        rk[12] &= 252;

        let t0 = u32::from_le_bytes([rk[0], rk[1], rk[2], rk[3]]);
        let t1 = u32::from_le_bytes([rk[3], rk[4], rk[5], rk[6]]) >> 2;
        let t2 = u32::from_le_bytes([rk[6], rk[7], rk[8], rk[9]]) >> 4;
        let t3 = u32::from_le_bytes([rk[9], rk[10], rk[11], rk[12]]) >> 6;
        let t4 = u32::from_le_bytes([rk[12], rk[13], rk[14], rk[15]]) >> 8;

        let r = [
            t0 & 0x3ff_ffff,
            t1 & 0x3ff_ffff,
            t2 & 0x3ff_ffff,
            t3 & 0x3ff_ffff,
            t4 & 0x3ff_ffff,
        ];
        let s = [r[1] * 5, r[2] * 5, r[3] * 5, r[4] * 5];
        let pad = [
            u32::from_le_bytes([key[16], key[17], key[18], key[19]]),
            u32::from_le_bytes([key[20], key[21], key[22], key[23]]),
            u32::from_le_bytes([key[24], key[25], key[26], key[27]]),
            u32::from_le_bytes([key[28], key[29], key[30], key[31]]),
        ];
        Self {
            r,
            s,
            h: [0; 5],
            pad,
        }
    }

    fn block(&mut self, block: &[u8], is_partial: bool) {
        let mut buf = [0u8; 16];
        buf[..block.len()].copy_from_slice(block);
        if is_partial {
            buf[block.len()] = 1;
        }
        let hibit: u32 = if is_partial { 0 } else { 1 << 24 };

        let t0 = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let t1 = u32::from_le_bytes([buf[3], buf[4], buf[5], buf[6]]) >> 2;
        let t2 = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]) >> 4;
        let t3 = u32::from_le_bytes([buf[9], buf[10], buf[11], buf[12]]) >> 6;
        let t4 = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]) >> 8;

        let h0 = (self.h[0] + (t0 & 0x3ff_ffff)) as u64;
        let h1 = (self.h[1] + (t1 & 0x3ff_ffff)) as u64;
        let h2 = (self.h[2] + (t2 & 0x3ff_ffff)) as u64;
        let h3 = (self.h[3] + (t3 & 0x3ff_ffff)) as u64;
        let h4 = (self.h[4] + (t4 & 0x3ff_ffff) + hibit) as u64;

        let r0 = self.r[0] as u64;
        let r1 = self.r[1] as u64;
        let r2 = self.r[2] as u64;
        let r3 = self.r[3] as u64;
        let r4 = self.r[4] as u64;
        let s1 = self.s[0] as u64;
        let s2 = self.s[1] as u64;
        let s3 = self.s[2] as u64;
        let s4 = self.s[3] as u64;

        let d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
        let mut d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
        let mut d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
        let mut d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
        let mut d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

        let mut c = d0 >> 26;
        let mut nh0 = (d0 & 0x3ff_ffff) as u32;
        d1 += c;
        c = d1 >> 26;
        let mut nh1 = (d1 & 0x3ff_ffff) as u32;
        d2 += c;
        c = d2 >> 26;
        let nh2 = (d2 & 0x3ff_ffff) as u32;
        d3 += c;
        c = d3 >> 26;
        let nh3 = (d3 & 0x3ff_ffff) as u32;
        d4 += c;
        c = d4 >> 26;
        let nh4 = (d4 & 0x3ff_ffff) as u32;
        let nh0_carry = nh0 as u64 + c * 5;
        nh0 = (nh0_carry & 0x3ff_ffff) as u32;
        nh1 += (nh0_carry >> 26) as u32;

        self.h = [nh0, nh1, nh2, nh3, nh4];
    }

    fn update_padded16(&mut self, data: &[u8]) {
        for chunk in data.chunks(16) {
            if chunk.len() == 16 {
                self.block(chunk, false);
            } else {
                let mut padded = [0u8; 16];
                padded[..chunk.len()].copy_from_slice(chunk);
                self.block(&padded, false);
            }
        }
    }

    fn finish(mut self) -> [u8; 16] {
        let mut c = self.h[1] >> 26;
        self.h[1] &= 0x3ff_ffff;
        self.h[2] += c;
        c = self.h[2] >> 26;
        self.h[2] &= 0x3ff_ffff;
        self.h[3] += c;
        c = self.h[3] >> 26;
        self.h[3] &= 0x3ff_ffff;
        self.h[4] += c;
        c = self.h[4] >> 26;
        self.h[4] &= 0x3ff_ffff;
        self.h[0] += c * 5;
        c = self.h[0] >> 26;
        self.h[0] &= 0x3ff_ffff;
        self.h[1] += c;

        let mut g = [0u32; 5];
        g[0] = self.h[0].wrapping_add(5);
        c = g[0] >> 26;
        g[0] &= 0x3ff_ffff;
        g[1] = self.h[1].wrapping_add(c);
        c = g[1] >> 26;
        g[1] &= 0x3ff_ffff;
        g[2] = self.h[2].wrapping_add(c);
        c = g[2] >> 26;
        g[2] &= 0x3ff_ffff;
        g[3] = self.h[3].wrapping_add(c);
        c = g[3] >> 26;
        g[3] &= 0x3ff_ffff;
        g[4] = self.h[4].wrapping_add(c).wrapping_sub(1 << 26);

        let mask = (g[4] >> 31).wrapping_sub(1);
        for i in 0..5 {
            self.h[i] = (self.h[i] & !mask) | (g[i] & mask);
        }

        let mut f0 = ((self.h[0]) | (self.h[1] << 26)) as u64 + self.pad[0] as u64;
        let f1 = ((self.h[1] >> 6) | (self.h[2] << 20)) as u64 + self.pad[1] as u64 + (f0 >> 32);
        let f2 = ((self.h[2] >> 12) | (self.h[3] << 14)) as u64 + self.pad[2] as u64 + (f1 >> 32);
        let f3 = ((self.h[3] >> 18) | (self.h[4] << 8)) as u64 + self.pad[3] as u64 + (f2 >> 32);
        f0 &= 0xFFFF_FFFF;

        let mut tag = [0u8; 16];
        tag[0..4].copy_from_slice(&(f0 as u32).to_le_bytes());
        tag[4..8].copy_from_slice(&(f1 as u32).to_le_bytes());
        tag[8..12].copy_from_slice(&(f2 as u32).to_le_bytes());
        tag[12..16].copy_from_slice(&(f3 as u32).to_le_bytes());
        tag
    }
}

fn poly1305_aead_tag(otk: &[u8; 32], aad: &[u8], ct: &[u8]) -> [u8; 16] {
    let mut mac = Poly1305::new(otk);
    mac.update_padded16(aad);
    mac.update_padded16(ct);
    let mut lens = [0u8; 16];
    lens[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lens[8..16].copy_from_slice(&(ct.len() as u64).to_le_bytes());
    mac.block(&lens, false);
    mac.finish()
}

pub(crate) fn chacha20_poly1305_seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    plaintext: &[u8],
    aad: &[u8],
) -> Vec<u8> {
    let otk_block = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&otk_block[..32]);
    let mut out = chacha20_xor(key, 1, nonce, plaintext);
    let tag = poly1305_aead_tag(&otk, aad, &out);
    out.extend_from_slice(&tag);
    out
}

pub(crate) fn chacha20_poly1305_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    sealed: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, String> {
    if sealed.len() < 16 {
        return Err("crypto.open(): ciphertext is shorter than 16-byte tag".to_string());
    }
    let split = sealed.len() - 16;
    let ct = &sealed[..split];
    let expected_tag = &sealed[split..];

    let otk_block = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&otk_block[..32]);
    let actual_tag = poly1305_aead_tag(&otk, aad, ct);
    if !ct_eq_bytes(&actual_tag, expected_tag) {
        return Err("crypto.open(): authentication failed".to_string());
    }
    Ok(chacha20_xor(key, 1, nonce, ct))
}

fn crypto_hmac_sha256(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("crypto.hmac_sha256() expects 2 arguments (key, data)".to_string());
    }
    let key = args[0].as_bytes_cow("crypto.hmac_sha256()")?;
    let data = args[1].as_bytes_cow("crypto.hmac_sha256()")?;
    if key.len() > MAX_CRYPTO_INPUT || data.len() > MAX_CRYPTO_INPUT {
        return Err("crypto.hmac_sha256(): input exceeds 32 MiB limit".to_string());
    }
    vm.charge_work(((key.len() + data.len()) as u64) / 32 + 4)?;
    vm.charge_alloc(32)?;
    Ok(Value::Bytes(Rc::new(hmac_sha256_raw(&key, &data).to_vec())))
}

fn crypto_hmac_sha256_hex(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("crypto.hmac_sha256_hex() expects 2 arguments (key, data)".to_string());
    }
    let key = args[0].as_bytes_cow("crypto.hmac_sha256_hex()")?;
    let data = args[1].as_bytes_cow("crypto.hmac_sha256_hex()")?;
    if key.len() > MAX_CRYPTO_INPUT || data.len() > MAX_CRYPTO_INPUT {
        return Err("crypto.hmac_sha256_hex(): input exceeds 32 MiB limit".to_string());
    }
    vm.charge_work(((key.len() + data.len()) as u64) / 32 + 4)?;
    vm.charge_alloc(64)?;
    let mac = hmac_sha256_raw(&key, &data);
    let mut hex = String::with_capacity(64);
    for b in mac {
        hex.push_str(&format!("{:02x}", b));
    }
    Ok(Value::from_str(hex))
}

fn crypto_constant_time_eq(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("crypto.constant_time_eq() expects 2 arguments (a, b)".to_string());
    }
    let a = args[0].as_bytes_cow("crypto.constant_time_eq()")?;
    let b = args[1].as_bytes_cow("crypto.constant_time_eq()")?;
    vm.charge_work((a.len().max(b.len()) as u64) / 32 + 1)?;
    Ok(Value::Bool(ct_eq_bytes(&a, &b)))
}

fn crypto_random_key(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("crypto.random_key() expects 0 arguments".to_string());
    }
    vm.charge_alloc(32)?;
    vm.charge_work(4)?;
    let mut buf = vec![0u8; 32];
    os_random_bytes(&mut buf)?;
    Ok(Value::Bytes(Rc::new(buf)))
}

fn crypto_random_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("crypto.random_bytes() expects 1 integer argument (count)".to_string());
    }
    let count = match &args[0] {
        Value::Int(n) if *n >= 0 && *n <= 1024 * 1024 => *n as usize,
        Value::Int(_) => {
            return Err("crypto.random_bytes() count must be in 0..=1048576".to_string());
        }
        other => {
            return Err(format!(
                "crypto.random_bytes() expects an integer, got '{}'",
                other.type_name()
            ));
        }
    };
    vm.charge_alloc(count)?;
    vm.charge_work((count as u64) / 32 + 2)?;
    let mut buf = vec![0u8; count];
    if count > 0 {
        os_random_bytes(&mut buf)?;
    }
    Ok(Value::Bytes(Rc::new(buf)))
}

fn crypto_seal(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("crypto.seal() expects (key_32, nonce_12, plaintext, [aad])".to_string());
    }
    let key_cow = args[0].as_bytes_cow("crypto.seal()")?;
    let nonce_cow = args[1].as_bytes_cow("crypto.seal()")?;
    let pt_cow = args[2].as_bytes_cow("crypto.seal()")?;
    let aad_cow = if args.len() == 4 {
        Some(args[3].as_bytes_cow("crypto.seal()")?)
    } else {
        None
    };

    if key_cow.len() != 32 {
        return Err(format!(
            "crypto.seal(): key must be 32 bytes, got {}",
            key_cow.len()
        ));
    }
    if nonce_cow.len() != 12 {
        return Err(format!(
            "crypto.seal(): nonce must be 12 bytes, got {}",
            nonce_cow.len()
        ));
    }
    let aad: &[u8] = aad_cow.as_deref().unwrap_or(&[]);
    if pt_cow.len() > MAX_CRYPTO_INPUT || aad.len() > MAX_CRYPTO_INPUT {
        return Err("crypto.seal(): payload exceeds 32 MiB limit".to_string());
    }
    vm.charge_alloc(pt_cow.len().saturating_add(16))?;
    vm.charge_work(((pt_cow.len() + aad.len()) as u64) / 16 + 8)?;

    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    key.copy_from_slice(&key_cow);
    nonce.copy_from_slice(&nonce_cow);
    Ok(Value::Bytes(Rc::new(chacha20_poly1305_seal(
        &key, &nonce, &pt_cow, aad,
    ))))
}

fn crypto_open(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("crypto.open() expects (key_32, nonce_12, ciphertext, [aad])".to_string());
    }
    let key_cow = args[0].as_bytes_cow("crypto.open()")?;
    let nonce_cow = args[1].as_bytes_cow("crypto.open()")?;
    let ct_cow = args[2].as_bytes_cow("crypto.open()")?;
    let aad_cow = if args.len() == 4 {
        Some(args[3].as_bytes_cow("crypto.open()")?)
    } else {
        None
    };

    if key_cow.len() != 32 {
        return Err(format!(
            "crypto.open(): key must be 32 bytes, got {}",
            key_cow.len()
        ));
    }
    if nonce_cow.len() != 12 {
        return Err(format!(
            "crypto.open(): nonce must be 12 bytes, got {}",
            nonce_cow.len()
        ));
    }
    let aad: &[u8] = aad_cow.as_deref().unwrap_or(&[]);
    if ct_cow.len() > MAX_CRYPTO_INPUT + 16 || aad.len() > MAX_CRYPTO_INPUT {
        return Err("crypto.open(): payload exceeds size limit".to_string());
    }
    vm.charge_alloc(ct_cow.len())?;
    vm.charge_work(((ct_cow.len() + aad.len()) as u64) / 16 + 8)?;

    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    key.copy_from_slice(&key_cow);
    nonce.copy_from_slice(&nonce_cow);
    let pt = chacha20_poly1305_open(&key, &nonce, &ct_cow, aad)?;
    Ok(Value::Bytes(Rc::new(pt)))
}
