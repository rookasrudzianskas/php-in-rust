//! xxHash: `xxh32`, `xxh64`, `xxh3` (64-bit) and `xxh128`.

use crate::hasher::{Engine, HashError, load_le32, load_le64};
use crate::state::{StateResult, Visitor};
use crate::{XXH3_SECRET_SIZE_MAX, XXH3_SECRET_SIZE_MIN};

const PRIME32_1: u32 = 0x9E37_79B1;
const PRIME32_2: u32 = 0x85EB_CA77;
const PRIME32_3: u32 = 0xC2B2_AE3D;
const PRIME32_4: u32 = 0x27D4_EB2F;
const PRIME32_5: u32 = 0x1656_67B1;

const PRIME64_1: u64 = 0x9E37_79B1_85EB_CA87;
const PRIME64_2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const PRIME64_3: u64 = 0x1656_67B1_9E37_79F9;
const PRIME64_4: u64 = 0x85EB_CA77_C2B2_AE63;
const PRIME64_5: u64 = 0x27D4_EB2F_1656_67C5;

fn read32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn read64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

// -------------------------------------------------------------------- XXH32

fn round32(acc: u32, input: u32) -> u32 {
    acc.wrapping_add(input.wrapping_mul(PRIME32_2))
        .rotate_left(13)
        .wrapping_mul(PRIME32_1)
}

/// xxHash 32-bit (`xxh32`); the field layout is `XXH32_state_t`.
#[derive(Clone)]
pub(crate) struct Xxh32 {
    total_len: u32,
    large_len: u32,
    v: [u32; 4],
    mem: [u8; 16],
    memsize: u32,
    reserved: u32,
}

impl Xxh32 {
    pub(crate) fn new(seed: Option<i64>) -> Self {
        let seed = seed.unwrap_or(0) as u32;
        Self {
            total_len: 0,
            large_len: 0,
            v: [
                seed.wrapping_add(PRIME32_1).wrapping_add(PRIME32_2),
                seed.wrapping_add(PRIME32_2),
                seed,
                seed.wrapping_sub(PRIME32_1),
            ],
            mem: [0; 16],
            memsize: 0,
            reserved: 0,
        }
    }

    fn consume(&mut self, block: &[u8]) {
        for (i, v) in self.v.iter_mut().enumerate() {
            *v = round32(*v, read32(&block[i * 4..]));
        }
    }
}

impl Engine for Xxh32 {
    fn update(&mut self, data: &[u8]) {
        let len = data.len();
        self.total_len = self.total_len.wrapping_add(len as u32);
        self.large_len |= u32::from(len >= 16 || self.total_len >= 16);
        let memsize = self.memsize as usize;
        if memsize + len < 16 {
            self.mem[memsize..memsize + len].copy_from_slice(data);
            self.memsize += len as u32;
            return;
        }
        let mut p = data;
        if memsize != 0 {
            let fill = 16 - memsize;
            self.mem[memsize..].copy_from_slice(&p[..fill]);
            let mem = self.mem;
            self.consume(&mem);
            p = &p[fill..];
            self.memsize = 0;
        }
        while let Some((block, rest)) = p.split_first_chunk::<16>() {
            self.consume(block);
            p = rest;
        }
        if !p.is_empty() {
            self.mem[..p.len()].copy_from_slice(p);
            self.memsize = p.len() as u32;
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut h = if self.large_len != 0 {
            self.v[0]
                .rotate_left(1)
                .wrapping_add(self.v[1].rotate_left(7))
                .wrapping_add(self.v[2].rotate_left(12))
                .wrapping_add(self.v[3].rotate_left(18))
        } else {
            self.v[2].wrapping_add(PRIME32_5)
        };
        h = h.wrapping_add(self.total_len);
        let mut tail = &self.mem[..self.memsize as usize];
        while let Some((c, rest)) = tail.split_first_chunk::<4>() {
            h = h
                .wrapping_add(u32::from_le_bytes(*c).wrapping_mul(PRIME32_3))
                .rotate_left(17)
                .wrapping_mul(PRIME32_4);
            tail = rest;
        }
        for &b in tail {
            h = h
                .wrapping_add(u32::from(b).wrapping_mul(PRIME32_5))
                .rotate_left(11)
                .wrapping_mul(PRIME32_1);
        }
        h ^= h >> 15;
        h = h.wrapping_mul(PRIME32_2);
        h ^= h >> 13;
        h = h.wrapping_mul(PRIME32_3);
        h ^= h >> 16;
        h.to_be_bytes().to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32(&mut self.total_len)?;
        v.u32(&mut self.large_len)?;
        v.u32s(&mut self.v)?;
        let mut words = load_le32::<4>(&self.mem);
        v.u32s(&mut words)?;
        for (i, w) in words.iter().enumerate() {
            self.mem[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        v.u32(&mut self.memsize)?;
        v.u32(&mut self.reserved)
    }

    fn validate(&self) -> bool {
        self.memsize < 16
    }
}

// -------------------------------------------------------------------- XXH64

fn round64(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(PRIME64_2))
        .rotate_left(31)
        .wrapping_mul(PRIME64_1)
}

fn merge_round64(acc: u64, val: u64) -> u64 {
    (acc ^ round64(0, val))
        .wrapping_mul(PRIME64_1)
        .wrapping_add(PRIME64_4)
}

fn avalanche64(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(PRIME64_2);
    h ^= h >> 29;
    h = h.wrapping_mul(PRIME64_3);
    h ^ (h >> 32)
}

/// xxHash 64-bit (`xxh64`); the field layout is `XXH64_state_t`.
#[derive(Clone)]
pub(crate) struct Xxh64 {
    total_len: u64,
    v: [u64; 4],
    mem: [u8; 32],
    memsize: u32,
    reserved32: u32,
    reserved64: u64,
}

impl Xxh64 {
    pub(crate) fn new(seed: Option<i64>) -> Self {
        let seed = seed.unwrap_or(0) as u64;
        Self {
            total_len: 0,
            v: [
                seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2),
                seed.wrapping_add(PRIME64_2),
                seed,
                seed.wrapping_sub(PRIME64_1),
            ],
            mem: [0; 32],
            memsize: 0,
            reserved32: 0,
            reserved64: 0,
        }
    }

    fn consume(&mut self, block: &[u8]) {
        for (i, v) in self.v.iter_mut().enumerate() {
            *v = round64(*v, read64(&block[i * 8..]));
        }
    }
}

impl Engine for Xxh64 {
    fn update(&mut self, data: &[u8]) {
        let len = data.len();
        self.total_len = self.total_len.wrapping_add(len as u64);
        let memsize = self.memsize as usize;
        if memsize + len < 32 {
            self.mem[memsize..memsize + len].copy_from_slice(data);
            self.memsize += len as u32;
            return;
        }
        let mut p = data;
        if memsize != 0 {
            let fill = 32 - memsize;
            self.mem[memsize..].copy_from_slice(&p[..fill]);
            let mem = self.mem;
            self.consume(&mem);
            p = &p[fill..];
            self.memsize = 0;
        }
        while let Some((block, rest)) = p.split_first_chunk::<32>() {
            self.consume(block);
            p = rest;
        }
        if !p.is_empty() {
            self.mem[..p.len()].copy_from_slice(p);
            self.memsize = p.len() as u32;
        }
    }

    fn finish(self) -> Vec<u8> {
        let [v1, v2, v3, v4] = self.v;
        let mut h = if self.total_len >= 32 {
            let mut h = v1
                .rotate_left(1)
                .wrapping_add(v2.rotate_left(7))
                .wrapping_add(v3.rotate_left(12))
                .wrapping_add(v4.rotate_left(18));
            h = merge_round64(h, v1);
            h = merge_round64(h, v2);
            h = merge_round64(h, v3);
            merge_round64(h, v4)
        } else {
            v3.wrapping_add(PRIME64_5)
        };
        h = h.wrapping_add(self.total_len);
        // PHP/xxHash take the tail length from `total_len & 31`; it only differs
        // from `memsize` for hand-crafted deserialised states.
        let mut rest = &self.mem[..(self.total_len & 31) as usize];
        while let Some((c, tail)) = rest.split_first_chunk::<8>() {
            h ^= round64(0, u64::from_le_bytes(*c));
            h = h
                .rotate_left(27)
                .wrapping_mul(PRIME64_1)
                .wrapping_add(PRIME64_4);
            rest = tail;
        }
        if rest.len() >= 4 {
            h ^= u64::from(read32(rest)).wrapping_mul(PRIME64_1);
            h = h
                .rotate_left(23)
                .wrapping_mul(PRIME64_2)
                .wrapping_add(PRIME64_3);
            rest = &rest[4..];
        }
        for &b in rest {
            h ^= u64::from(b).wrapping_mul(PRIME64_5);
            h = h.rotate_left(11).wrapping_mul(PRIME64_1);
        }
        avalanche64(h).to_be_bytes().to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u64(&mut self.total_len)?;
        v.u64s(&mut self.v)?;
        let mut words = load_le64::<4>(&self.mem);
        v.u64s(&mut words)?;
        for (i, w) in words.iter().enumerate() {
            self.mem[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
        }
        v.u32(&mut self.memsize)?;
        v.u32(&mut self.reserved32)?;
        v.u64(&mut self.reserved64)
    }

    fn validate(&self) -> bool {
        self.memsize < 32
    }
}

// --------------------------------------------------------------------- XXH3

const STRIPE_LEN: usize = 64;
const SECRET_CONSUME_RATE: usize = 8;
const INTERNAL_BUFFER_SIZE: usize = 256;
const MIDSIZE_MAX: usize = 240;
const MIDSIZE_START_OFFSET: usize = 3;
const MIDSIZE_LAST_OFFSET: usize = 17;
const SECRET_MERGEACCS_START: usize = 11;
const SECRET_LASTACC_START: usize = 7;
const PRIME_MX1: u64 = 0x1656_6791_9E37_79F9;
const PRIME_MX2: u64 = 0x9FB2_1C65_1E98_DF25;

const INIT_ACC: [u64; 8] = [
    PRIME32_3 as u64,
    PRIME64_1,
    PRIME64_2,
    PRIME64_3,
    PRIME64_4,
    PRIME32_2 as u64,
    PRIME64_5,
    PRIME32_1 as u64,
];

fn xorshift64(v: u64, shift: u32) -> u64 {
    v ^ (v >> shift)
}

fn avalanche3(mut h: u64) -> u64 {
    h = xorshift64(h, 37);
    h = h.wrapping_mul(PRIME_MX1);
    xorshift64(h, 32)
}

fn rrmxmx(mut h: u64, len: u64) -> u64 {
    h ^= h.rotate_left(49) ^ h.rotate_left(24);
    h = h.wrapping_mul(PRIME_MX2);
    h ^= (h >> 35).wrapping_add(len);
    h = h.wrapping_mul(PRIME_MX2);
    xorshift64(h, 28)
}

fn mul128(a: u64, b: u64) -> (u64, u64) {
    let p = u128::from(a) * u128::from(b);
    (p as u64, (p >> 64) as u64)
}

fn mul128_fold64(a: u64, b: u64) -> u64 {
    let (lo, hi) = mul128(a, b);
    lo ^ hi
}

fn mix16(input: &[u8], secret: &[u8], seed: u64) -> u64 {
    mul128_fold64(
        read64(input) ^ read64(secret).wrapping_add(seed),
        read64(&input[8..]) ^ read64(&secret[8..]).wrapping_sub(seed),
    )
}

fn len_1to3_64(input: &[u8], secret: &[u8], seed: u64) -> u64 {
    let len = input.len();
    let c1 = u32::from(input[0]);
    let c2 = u32::from(input[len >> 1]);
    let c3 = u32::from(input[len - 1]);
    let combined = (c1 << 16) | (c2 << 24) | c3 | ((len as u32) << 8);
    let bitflip = u64::from(read32(secret) ^ read32(&secret[4..])).wrapping_add(seed);
    avalanche64(u64::from(combined) ^ bitflip)
}

fn len_4to8_64(input: &[u8], secret: &[u8], seed: u64) -> u64 {
    let len = input.len();
    let seed = seed ^ (u64::from((seed as u32).swap_bytes()) << 32);
    let input1 = read32(input);
    let input2 = read32(&input[len - 4..]);
    let bitflip = (read64(&secret[8..]) ^ read64(&secret[16..])).wrapping_sub(seed);
    let input64 = u64::from(input2).wrapping_add(u64::from(input1) << 32);
    rrmxmx(input64 ^ bitflip, len as u64)
}

fn len_9to16_64(input: &[u8], secret: &[u8], seed: u64) -> u64 {
    let len = input.len();
    let bitflip1 = (read64(&secret[24..]) ^ read64(&secret[32..])).wrapping_add(seed);
    let bitflip2 = (read64(&secret[40..]) ^ read64(&secret[48..])).wrapping_sub(seed);
    let lo = read64(input) ^ bitflip1;
    let hi = read64(&input[len - 8..]) ^ bitflip2;
    let acc = (len as u64)
        .wrapping_add(lo.swap_bytes())
        .wrapping_add(hi)
        .wrapping_add(mul128_fold64(lo, hi));
    avalanche3(acc)
}

fn len_0to16_64(input: &[u8], secret: &[u8], seed: u64) -> u64 {
    match input.len() {
        0 => avalanche64(seed ^ (read64(&secret[56..]) ^ read64(&secret[64..]))),
        1..=3 => len_1to3_64(input, secret, seed),
        4..=8 => len_4to8_64(input, secret, seed),
        _ => len_9to16_64(input, secret, seed),
    }
}

fn len_17to128_64(input: &[u8], secret: &[u8], seed: u64) -> u64 {
    let len = input.len();
    let mut acc = (len as u64).wrapping_mul(PRIME64_1);
    if len > 32 {
        if len > 64 {
            if len > 96 {
                acc = acc.wrapping_add(mix16(&input[48..], &secret[96..], seed));
                acc = acc.wrapping_add(mix16(&input[len - 64..], &secret[112..], seed));
            }
            acc = acc.wrapping_add(mix16(&input[32..], &secret[64..], seed));
            acc = acc.wrapping_add(mix16(&input[len - 48..], &secret[80..], seed));
        }
        acc = acc.wrapping_add(mix16(&input[16..], &secret[32..], seed));
        acc = acc.wrapping_add(mix16(&input[len - 32..], &secret[48..], seed));
    }
    acc = acc.wrapping_add(mix16(input, secret, seed));
    acc = acc.wrapping_add(mix16(&input[len - 16..], &secret[16..], seed));
    avalanche3(acc)
}

fn len_129to240_64(input: &[u8], secret: &[u8], seed: u64) -> u64 {
    let len = input.len();
    let mut acc = (len as u64).wrapping_mul(PRIME64_1);
    let rounds = len / 16;
    for i in 0..8 {
        acc = acc.wrapping_add(mix16(&input[16 * i..], &secret[16 * i..], seed));
    }
    acc = avalanche3(acc);
    for i in 8..rounds {
        acc = acc.wrapping_add(mix16(
            &input[16 * i..],
            &secret[16 * (i - 8) + MIDSIZE_START_OFFSET..],
            seed,
        ));
    }
    acc = acc.wrapping_add(mix16(
        &input[len - 16..],
        &secret[XXH3_SECRET_SIZE_MIN - MIDSIZE_LAST_OFFSET..],
        seed,
    ));
    avalanche3(acc)
}

/// 128-bit result as (low64, high64).
type U128Pair = (u64, u64);

fn len_1to3_128(input: &[u8], secret: &[u8], seed: u64) -> U128Pair {
    let len = input.len();
    let c1 = u32::from(input[0]);
    let c2 = u32::from(input[len >> 1]);
    let c3 = u32::from(input[len - 1]);
    let combined_l = (c1 << 16) | (c2 << 24) | c3 | ((len as u32) << 8);
    let combined_h = combined_l.swap_bytes().rotate_left(13);
    let bitflip_l = u64::from(read32(secret) ^ read32(&secret[4..])).wrapping_add(seed);
    let bitflip_h = u64::from(read32(&secret[8..]) ^ read32(&secret[12..])).wrapping_sub(seed);
    (
        avalanche64(u64::from(combined_l) ^ bitflip_l),
        avalanche64(u64::from(combined_h) ^ bitflip_h),
    )
}

fn len_4to8_128(input: &[u8], secret: &[u8], seed: u64) -> U128Pair {
    let len = input.len();
    let seed = seed ^ (u64::from((seed as u32).swap_bytes()) << 32);
    let input_lo = read32(input);
    let input_hi = read32(&input[len - 4..]);
    let input64 = u64::from(input_lo).wrapping_add(u64::from(input_hi) << 32);
    let bitflip = (read64(&secret[16..]) ^ read64(&secret[24..])).wrapping_add(seed);
    let keyed = input64 ^ bitflip;
    let (mut low, mut high) = mul128(keyed, PRIME64_1.wrapping_add((len as u64) << 2));
    high = high.wrapping_add(low << 1);
    low ^= high >> 3;
    low = xorshift64(low, 35);
    low = low.wrapping_mul(PRIME_MX2);
    low = xorshift64(low, 28);
    (low, avalanche3(high))
}

fn len_9to16_128(input: &[u8], secret: &[u8], seed: u64) -> U128Pair {
    let len = input.len();
    let bitflip_l = (read64(&secret[32..]) ^ read64(&secret[40..])).wrapping_sub(seed);
    let bitflip_h = (read64(&secret[48..]) ^ read64(&secret[56..])).wrapping_add(seed);
    let input_lo = read64(input);
    let mut input_hi = read64(&input[len - 8..]);
    let (mut m_low, mut m_high) = mul128(input_lo ^ input_hi ^ bitflip_l, PRIME64_1);
    m_low = m_low.wrapping_add(((len - 1) as u64) << 54);
    input_hi ^= bitflip_h;
    m_high = m_high
        .wrapping_add(input_hi)
        .wrapping_add(u64::from(input_hi as u32).wrapping_mul(u64::from(PRIME32_2 - 1)));
    m_low ^= m_high.swap_bytes();
    let (h_low, mut h_high) = mul128(m_low, PRIME64_2);
    h_high = h_high.wrapping_add(m_high.wrapping_mul(PRIME64_2));
    (avalanche3(h_low), avalanche3(h_high))
}

fn len_0to16_128(input: &[u8], secret: &[u8], seed: u64) -> U128Pair {
    match input.len() {
        0 => {
            let bitflip_l = read64(&secret[64..]) ^ read64(&secret[72..]);
            let bitflip_h = read64(&secret[80..]) ^ read64(&secret[88..]);
            (avalanche64(seed ^ bitflip_l), avalanche64(seed ^ bitflip_h))
        }
        1..=3 => len_1to3_128(input, secret, seed),
        4..=8 => len_4to8_128(input, secret, seed),
        _ => len_9to16_128(input, secret, seed),
    }
}

fn mix32(acc: &mut U128Pair, input1: &[u8], input2: &[u8], secret: &[u8], seed: u64) {
    acc.0 = acc.0.wrapping_add(mix16(input1, secret, seed));
    acc.0 ^= read64(input2).wrapping_add(read64(&input2[8..]));
    acc.1 = acc.1.wrapping_add(mix16(input2, &secret[16..], seed));
    acc.1 ^= read64(input1).wrapping_add(read64(&input1[8..]));
}

fn finish_mid_128(acc: U128Pair, len: usize, seed: u64) -> U128Pair {
    let low = acc.0.wrapping_add(acc.1);
    let high = acc
        .0
        .wrapping_mul(PRIME64_1)
        .wrapping_add(acc.1.wrapping_mul(PRIME64_4))
        .wrapping_add((len as u64).wrapping_sub(seed).wrapping_mul(PRIME64_2));
    (avalanche3(low), 0u64.wrapping_sub(avalanche3(high)))
}

fn len_17to128_128(input: &[u8], secret: &[u8], seed: u64) -> U128Pair {
    let len = input.len();
    let mut acc: U128Pair = ((len as u64).wrapping_mul(PRIME64_1), 0);
    if len > 32 {
        if len > 64 {
            if len > 96 {
                mix32(
                    &mut acc,
                    &input[48..],
                    &input[len - 64..],
                    &secret[96..],
                    seed,
                );
            }
            mix32(
                &mut acc,
                &input[32..],
                &input[len - 48..],
                &secret[64..],
                seed,
            );
        }
        mix32(
            &mut acc,
            &input[16..],
            &input[len - 32..],
            &secret[32..],
            seed,
        );
    }
    mix32(&mut acc, input, &input[len - 16..], secret, seed);
    finish_mid_128(acc, len, seed)
}

fn len_129to240_128(input: &[u8], secret: &[u8], seed: u64) -> U128Pair {
    let len = input.len();
    let rounds = len / 32;
    let mut acc: U128Pair = ((len as u64).wrapping_mul(PRIME64_1), 0);
    for i in 0..4 {
        mix32(
            &mut acc,
            &input[32 * i..],
            &input[32 * i + 16..],
            &secret[32 * i..],
            seed,
        );
    }
    acc.0 = avalanche3(acc.0);
    acc.1 = avalanche3(acc.1);
    for i in 4..rounds {
        mix32(
            &mut acc,
            &input[32 * i..],
            &input[32 * i + 16..],
            &secret[MIDSIZE_START_OFFSET + 32 * (i - 4)..],
            seed,
        );
    }
    mix32(
        &mut acc,
        &input[len - 16..],
        &input[len - 32..],
        &secret[XXH3_SECRET_SIZE_MIN - MIDSIZE_LAST_OFFSET - 16..],
        0u64.wrapping_sub(seed),
    );
    finish_mid_128(acc, len, seed)
}

/// Accumulates one 64-byte stripe.
fn accumulate_512(acc: &mut [u64; 8], input: &[u8], secret: &[u8]) {
    for i in 0..8 {
        let data_val = read64(&input[8 * i..]);
        let data_key = data_val ^ read64(&secret[8 * i..]);
        acc[i ^ 1] = acc[i ^ 1].wrapping_add(data_val);
        acc[i] = acc[i].wrapping_add((data_key & 0xffff_ffff).wrapping_mul(data_key >> 32));
    }
}

fn accumulate(acc: &mut [u64; 8], input: &[u8], secret: &[u8], stripes: usize) {
    for n in 0..stripes {
        accumulate_512(
            acc,
            &input[n * STRIPE_LEN..],
            &secret[n * SECRET_CONSUME_RATE..],
        );
    }
}

fn scramble(acc: &mut [u64; 8], secret: &[u8]) {
    for (i, a) in acc.iter_mut().enumerate() {
        let key = read64(&secret[8 * i..]);
        *a = xorshift64(*a, 47) ^ key;
        *a = a.wrapping_mul(u64::from(PRIME32_1));
    }
}

fn merge_accs(acc: &[u64; 8], secret: &[u8], start: u64) -> u64 {
    let mut result = start;
    for i in 0..4 {
        result = result.wrapping_add(mul128_fold64(
            acc[2 * i] ^ read64(&secret[16 * i..]),
            acc[2 * i + 1] ^ read64(&secret[16 * i + 8..]),
        ));
    }
    avalanche3(result)
}

fn init_custom_secret(seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(192);
    for i in 0..12 {
        let lo = read64(&K_SECRET[16 * i..]).wrapping_add(seed);
        let hi = read64(&K_SECRET[16 * i + 8..]).wrapping_sub(seed);
        out.extend_from_slice(&lo.to_le_bytes());
        out.extend_from_slice(&hi.to_le_bytes());
    }
    out
}

/// xxh3 / xxh128 streaming state (`XXH3_state_t`).  PHP cannot serialise it.
#[derive(Clone)]
pub(crate) struct Xxh3 {
    wide: bool,
    acc: [u64; 8],
    /// Secret used by the long-input loop (default, derived from the seed, or user supplied).
    secret: Vec<u8>,
    seed: u64,
    /// Short inputs use the user secret directly (seed 0) or `K_SECRET` with the seed.
    custom_secret: bool,
    buffer: [u8; INTERNAL_BUFFER_SIZE],
    buffered: usize,
    stripes_so_far: usize,
    total_len: u64,
}

impl Xxh3 {
    pub(crate) fn new(
        algo: &'static str,
        wide: bool,
        seed: Option<i64>,
        secret: Option<&[u8]>,
    ) -> Result<Self, HashError> {
        let (seed, secret_vec, custom) = match (seed, secret) {
            (Some(_), Some(_)) => return Err(HashError::SeedAndSecret { algo }),
            (_, Some(s)) => {
                if s.len() < XXH3_SECRET_SIZE_MIN {
                    return Err(HashError::SecretTooShort { algo, len: s.len() });
                }
                (0, s[..s.len().min(XXH3_SECRET_SIZE_MAX)].to_vec(), true)
            }
            (Some(seed), None) if seed != 0 => {
                let seed = seed as u64;
                (seed, init_custom_secret(seed), false)
            }
            _ => (0, K_SECRET.to_vec(), false),
        };
        Ok(Self {
            wide,
            acc: INIT_ACC,
            secret: secret_vec,
            seed,
            custom_secret: custom,
            buffer: [0; INTERNAL_BUFFER_SIZE],
            buffered: 0,
            stripes_so_far: 0,
            total_len: 0,
        })
    }

    fn stripes_per_block(&self) -> usize {
        (self.secret.len() - STRIPE_LEN) / SECRET_CONSUME_RATE
    }

    fn secret_limit(&self) -> usize {
        self.secret.len() - STRIPE_LEN
    }

    fn consume_stripes(
        acc: &mut [u64; 8],
        so_far: &mut usize,
        per_block: usize,
        input: &[u8],
        stripes: usize,
        secret: &[u8],
        secret_limit: usize,
    ) {
        if per_block - *so_far <= stripes {
            let to_end = per_block - *so_far;
            let after = stripes - to_end;
            accumulate(acc, input, &secret[*so_far * SECRET_CONSUME_RATE..], to_end);
            scramble(acc, &secret[secret_limit..]);
            accumulate(acc, &input[to_end * STRIPE_LEN..], secret, after);
            *so_far = after;
        } else {
            accumulate(
                acc,
                input,
                &secret[*so_far * SECRET_CONSUME_RATE..],
                stripes,
            );
            *so_far += stripes;
        }
    }

    /// The accumulators with all buffered data folded in (`XXH3_digest_long`).
    fn digest_acc(&self) -> [u64; 8] {
        let mut acc = self.acc;
        let mut last_stripe = [0u8; STRIPE_LEN];
        let last: &[u8] = if self.buffered >= STRIPE_LEN {
            let stripes = (self.buffered - 1) / STRIPE_LEN;
            let mut so_far = self.stripes_so_far;
            Self::consume_stripes(
                &mut acc,
                &mut so_far,
                self.stripes_per_block(),
                &self.buffer,
                stripes,
                &self.secret,
                self.secret_limit(),
            );
            &self.buffer[self.buffered - STRIPE_LEN..self.buffered]
        } else {
            let catchup = STRIPE_LEN - self.buffered;
            last_stripe[..catchup].copy_from_slice(&self.buffer[INTERNAL_BUFFER_SIZE - catchup..]);
            last_stripe[catchup..].copy_from_slice(&self.buffer[..self.buffered]);
            &last_stripe
        };
        accumulate_512(
            &mut acc,
            last,
            &self.secret[self.secret_limit() - SECRET_LASTACC_START..],
        );
        acc
    }

    fn short_64(&self, input: &[u8]) -> u64 {
        let (secret, seed): (&[u8], u64) = if self.custom_secret {
            (&self.secret, 0)
        } else {
            (&K_SECRET, self.seed)
        };
        match input.len() {
            0..=16 => len_0to16_64(input, secret, seed),
            17..=128 => len_17to128_64(input, secret, seed),
            _ => len_129to240_64(input, secret, seed),
        }
    }

    fn short_128(&self, input: &[u8]) -> U128Pair {
        let (secret, seed): (&[u8], u64) = if self.custom_secret {
            (&self.secret, 0)
        } else {
            (&K_SECRET, self.seed)
        };
        match input.len() {
            0..=16 => len_0to16_128(input, secret, seed),
            17..=128 => len_17to128_128(input, secret, seed),
            _ => len_129to240_128(input, secret, seed),
        }
    }
}

impl Engine for Xxh3 {
    const MAGIC: Option<i64> = None;

    fn update(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if data.len() <= INTERNAL_BUFFER_SIZE - self.buffered {
            self.buffer[self.buffered..self.buffered + data.len()].copy_from_slice(data);
            self.buffered += data.len();
            return;
        }
        let per_block = self.stripes_per_block();
        let secret_limit = self.secret_limit();
        let mut pos = 0;
        if self.buffered > 0 {
            let load = INTERNAL_BUFFER_SIZE - self.buffered;
            self.buffer[self.buffered..].copy_from_slice(&data[..load]);
            pos = load;
            let buffer = self.buffer;
            Self::consume_stripes(
                &mut self.acc,
                &mut self.stripes_so_far,
                per_block,
                &buffer,
                INTERNAL_BUFFER_SIZE / STRIPE_LEN,
                &self.secret,
                secret_limit,
            );
            self.buffered = 0;
        }
        if pos + INTERNAL_BUFFER_SIZE < data.len() {
            let limit = data.len() - INTERNAL_BUFFER_SIZE;
            loop {
                Self::consume_stripes(
                    &mut self.acc,
                    &mut self.stripes_so_far,
                    per_block,
                    &data[pos..pos + INTERNAL_BUFFER_SIZE],
                    INTERNAL_BUFFER_SIZE / STRIPE_LEN,
                    &self.secret,
                    secret_limit,
                );
                pos += INTERNAL_BUFFER_SIZE;
                if pos >= limit {
                    break;
                }
            }
            // Keep the last stripe: needed when the final buffer is short.
            self.buffer[INTERNAL_BUFFER_SIZE - STRIPE_LEN..]
                .copy_from_slice(&data[pos - STRIPE_LEN..pos]);
        }
        let rest = &data[pos..];
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    fn finish(self) -> Vec<u8> {
        let total = self.total_len;
        if self.wide {
            let (low, high) = if total > MIDSIZE_MAX as u64 {
                let acc = self.digest_acc();
                let secret = &self.secret;
                (
                    merge_accs(
                        &acc,
                        &secret[SECRET_MERGEACCS_START..],
                        total.wrapping_mul(PRIME64_1),
                    ),
                    merge_accs(
                        &acc,
                        &secret[secret.len() - 64 - SECRET_MERGEACCS_START..],
                        !total.wrapping_mul(PRIME64_2),
                    ),
                )
            } else {
                self.short_128(&self.buffer[..self.buffered])
            };
            let mut out = high.to_be_bytes().to_vec();
            out.extend_from_slice(&low.to_be_bytes());
            out
        } else {
            let value = if total > MIDSIZE_MAX as u64 {
                let acc = self.digest_acc();
                merge_accs(
                    &acc,
                    &self.secret[SECRET_MERGEACCS_START..],
                    total.wrapping_mul(PRIME64_1),
                )
            } else {
                self.short_64(&self.buffer[..self.buffered])
            };
            value.to_be_bytes().to_vec()
        }
    }
}

const K_SECRET: [u8; 192] = [
    0xb8, 0xfe, 0x6c, 0x39, 0x23, 0xa4, 0x4b, 0xbe, 0x7c, 0x01, 0x81, 0x2c, 0xf7, 0x21, 0xad, 0x1c,
    0xde, 0xd4, 0x6d, 0xe9, 0x83, 0x90, 0x97, 0xdb, 0x72, 0x40, 0xa4, 0xa4, 0xb7, 0xb3, 0x67, 0x1f,
    0xcb, 0x79, 0xe6, 0x4e, 0xcc, 0xc0, 0xe5, 0x78, 0x82, 0x5a, 0xd0, 0x7d, 0xcc, 0xff, 0x72, 0x21,
    0xb8, 0x08, 0x46, 0x74, 0xf7, 0x43, 0x24, 0x8e, 0xe0, 0x35, 0x90, 0xe6, 0x81, 0x3a, 0x26, 0x4c,
    0x3c, 0x28, 0x52, 0xbb, 0x91, 0xc3, 0x00, 0xcb, 0x88, 0xd0, 0x65, 0x8b, 0x1b, 0x53, 0x2e, 0xa3,
    0x71, 0x64, 0x48, 0x97, 0xa2, 0x0d, 0xf9, 0x4e, 0x38, 0x19, 0xef, 0x46, 0xa9, 0xde, 0xac, 0xd8,
    0xa8, 0xfa, 0x76, 0x3f, 0xe3, 0x9c, 0x34, 0x3f, 0xf9, 0xdc, 0xbb, 0xc7, 0xc7, 0x0b, 0x4f, 0x1d,
    0x8a, 0x51, 0xe0, 0x4b, 0xcd, 0xb4, 0x59, 0x31, 0xc8, 0x9f, 0x7e, 0xc9, 0xd9, 0x78, 0x73, 0x64,
    0xea, 0xc5, 0xac, 0x83, 0x34, 0xd3, 0xeb, 0xc3, 0xc5, 0x81, 0xa0, 0xff, 0xfa, 0x13, 0x63, 0xeb,
    0x17, 0x0d, 0xdd, 0x51, 0xb7, 0xf0, 0xda, 0x49, 0xd3, 0x16, 0x55, 0x26, 0x29, 0xd4, 0x68, 0x9e,
    0x2b, 0x16, 0xbe, 0x58, 0x7d, 0x47, 0xa1, 0xfc, 0x8f, 0xf8, 0xb8, 0xd1, 0x7a, 0xd0, 0x31, 0xce,
    0x45, 0xcb, 0x3a, 0x8f, 0x95, 0x16, 0x04, 0x28, 0xaf, 0xd7, 0xfb, 0xca, 0xbb, 0x4b, 0x40, 0x7e,
];
