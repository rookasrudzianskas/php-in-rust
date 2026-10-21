//! MurmurHash3 as used by ext/hash: `murmur3a` (x86_32), `murmur3c` (x86_128)
//! and `murmur3f` (x64_128).
//!
//! PHP drives Shane Day's incremental `PMurHash` code: input is consumed in
//! whole words/blocks and the trailing bytes live in a "carry" shift register
//! whose exact contents (including stale bytes) show up in serialised contexts,
//! so the register handling below mirrors the C macros precisely.

use crate::hasher::{Engine, load_le32, load_le64};
use crate::state::{StateResult, Visitor};

fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

fn fmix64(mut k: u64) -> u64 {
    k ^= k >> 33;
    k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
    k ^= k >> 33;
    k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    k ^ (k >> 33)
}

// ---------------------------------------------------------------- murmur3a

const C1_32: u32 = 0xcc9e_2d51;
const C2_32: u32 = 0x1b87_3593;

/// Mixes one 32-bit block; `k1` is clobbered exactly like the C macro does.
fn block32(h1: &mut u32, k1: &mut u32) {
    *k1 = k1.wrapping_mul(C1_32);
    *k1 = k1.rotate_left(15);
    *k1 = k1.wrapping_mul(C2_32);
    *h1 ^= *k1;
    *h1 = h1.rotate_left(13);
    *h1 = h1.wrapping_mul(5).wrapping_add(0xe654_6b64);
}

/// MurmurHash3 x86_32 (`murmur3a`).
#[derive(Clone)]
pub(crate) struct Murmur3a {
    h: u32,
    carry: u32,
    len: u32,
}

impl Murmur3a {
    pub(crate) fn new(seed: Option<i64>) -> Self {
        Self {
            h: seed.unwrap_or(0) as u32,
            carry: 0,
            len: 0,
        }
    }

    fn shift_in(&mut self, n: &mut usize, byte: u8) {
        self.carry = (self.carry >> 8) | (u32::from(byte) << 24);
        *n += 1;
        if *n == 4 {
            let mut c = self.carry;
            block32(&mut self.h, &mut c);
            self.carry = c;
            *n = 0;
        }
    }
}

impl Engine for Murmur3a {
    fn update(&mut self, data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u32);
        let mut n = (self.carry & 3) as usize;
        let mut data = data;
        // Consume carry bytes so the next input byte is block aligned.
        let i = (4 - n) & 3;
        if i != 0 && i <= data.len() {
            for &b in &data[..i] {
                self.shift_in(&mut n, b);
            }
            data = &data[i..];
        }
        while let Some((chunk, rest)) = data.split_first_chunk::<4>() {
            let mut k1 = u32::from_le_bytes(*chunk);
            block32(&mut self.h, &mut k1);
            data = rest;
        }
        for &b in data {
            self.shift_in(&mut n, b);
        }
        self.carry = (self.carry & !0xff) | n as u32;
    }

    fn finish(self) -> Vec<u8> {
        let mut h = self.h;
        let n = (self.carry & 3) as usize;
        if n != 0 {
            let mut k1 = self.carry >> ((4 - n) * 8);
            k1 = k1.wrapping_mul(C1_32);
            k1 = k1.rotate_left(15);
            k1 = k1.wrapping_mul(C2_32);
            h ^= k1;
        }
        h ^= self.len;
        fmix32(h).to_be_bytes().to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32(&mut self.h)?;
        v.u32(&mut self.carry)?;
        v.u32(&mut self.len)
    }
}

// ---------------------------------------------------------------- murmur3c

const C32: [u32; 4] = [0x239b_961b, 0xab0e_9789, 0x38b3_4ae5, 0xa1e3_8b93];

/// Mixes one 128-bit block of the x86 variant; `k` is clobbered like the C macro.
fn block128x86(h: &mut [u32; 4], k: &mut [u32; 4]) {
    k[0] = k[0].wrapping_mul(C32[0]);
    k[0] = k[0].rotate_left(15);
    k[0] = k[0].wrapping_mul(C32[1]);
    h[0] ^= k[0];
    h[0] = h[0].rotate_left(19);
    h[0] = h[0].wrapping_add(h[1]);
    h[0] = h[0].wrapping_mul(5).wrapping_add(0x561c_cd1b);

    k[1] = k[1].wrapping_mul(C32[1]);
    k[1] = k[1].rotate_left(16);
    k[1] = k[1].wrapping_mul(C32[2]);
    h[1] ^= k[1];
    h[1] = h[1].rotate_left(17);
    h[1] = h[1].wrapping_add(h[2]);
    h[1] = h[1].wrapping_mul(5).wrapping_add(0x0bca_a747);

    k[2] = k[2].wrapping_mul(C32[2]);
    k[2] = k[2].rotate_left(17);
    k[2] = k[2].wrapping_mul(C32[3]);
    h[2] ^= k[2];
    h[2] = h[2].rotate_left(15);
    h[2] = h[2].wrapping_add(h[3]);
    h[2] = h[2].wrapping_mul(5).wrapping_add(0x96cd_1c35);

    k[3] = k[3].wrapping_mul(C32[3]);
    k[3] = k[3].rotate_left(18);
    k[3] = k[3].wrapping_mul(C32[0]);
    h[3] ^= k[3];
    h[3] = h[3].rotate_left(13);
    h[3] = h[3].wrapping_add(h[0]);
    h[3] = h[3].wrapping_mul(5).wrapping_add(0x32ac_3b17);
}

/// MurmurHash3 x86_128 (`murmur3c`).
#[derive(Clone)]
pub(crate) struct Murmur3c {
    h: [u32; 4],
    carry: [u32; 4],
    len: u32,
}

impl Murmur3c {
    pub(crate) fn new(seed: Option<i64>) -> Self {
        let s = seed.unwrap_or(0) as u32;
        Self {
            h: [s; 4],
            carry: [0; 4],
            len: 0,
        }
    }

    fn shift_in(&mut self, n: &mut usize, byte: u8) {
        let w = *n / 4;
        self.carry[w] = (self.carry[w] >> 8) | (u32::from(byte) << 24);
        if *n == 15 {
            let mut c = self.carry;
            block128x86(&mut self.h, &mut c);
            self.carry = c;
            *n = 0;
        } else {
            *n += 1;
        }
    }
}

impl Engine for Murmur3c {
    fn update(&mut self, data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u32);
        let mut n = (self.carry[3] & 15) as usize;
        let mut data = data;
        let i = (16 - n) & 15;
        if i != 0 && i <= data.len() {
            for &b in &data[..i] {
                self.shift_in(&mut n, b);
            }
            data = &data[i..];
        }
        // The carry registers double as the block variables (and keep the mixed values).
        while let Some((chunk, rest)) = data.split_first_chunk::<16>() {
            self.carry = load_le32(chunk);
            block128x86(&mut self.h, &mut self.carry);
            data = rest;
        }
        for &b in data {
            self.shift_in(&mut n, b);
        }
        self.carry[3] = (self.carry[3] & !0xff) | n as u32;
    }

    fn finish(self) -> Vec<u8> {
        let mut h = self.h;
        let n = (self.carry[3] & 15) as usize;
        // Tail: every carry word holding at least one byte is mixed in.
        let mixers: [(u32, u32, u32); 4] = [
            (15, C32[0], C32[1]),
            (16, C32[1], C32[2]),
            (17, C32[2], C32[3]),
            (18, C32[3], C32[0]),
        ];
        for (j, &(rot, ca, cb)) in mixers.iter().enumerate() {
            let have = n.saturating_sub(4 * j).min(4);
            if have > 0 {
                let mut k = self.carry[j] >> (32 - 8 * have as u32);
                k = k.wrapping_mul(ca);
                k = k.rotate_left(rot);
                k = k.wrapping_mul(cb);
                h[j] ^= k;
            }
        }
        for x in &mut h {
            *x ^= self.len;
        }
        h[0] = h[0]
            .wrapping_add(h[1])
            .wrapping_add(h[2])
            .wrapping_add(h[3]);
        h[1] = h[1].wrapping_add(h[0]);
        h[2] = h[2].wrapping_add(h[0]);
        h[3] = h[3].wrapping_add(h[0]);
        for x in &mut h {
            *x = fmix32(*x);
        }
        h[0] = h[0]
            .wrapping_add(h[1])
            .wrapping_add(h[2])
            .wrapping_add(h[3]);
        h[1] = h[1].wrapping_add(h[0]);
        h[2] = h[2].wrapping_add(h[0]);
        h[3] = h[3].wrapping_add(h[0]);
        h.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32s(&mut self.h)?;
        v.u32s(&mut self.carry)?;
        v.u32(&mut self.len)
    }
}

// ---------------------------------------------------------------- murmur3f

const C64: [u64; 2] = [0x87c3_7b91_1142_53d5, 0x4cf5_ad43_2745_937f];

fn block128x64(h: &mut [u64; 2], k: &mut [u64; 2]) {
    k[0] = k[0].wrapping_mul(C64[0]);
    k[0] = k[0].rotate_left(31);
    k[0] = k[0].wrapping_mul(C64[1]);
    h[0] ^= k[0];
    h[0] = h[0].rotate_left(27);
    h[0] = h[0].wrapping_add(h[1]);
    h[0] = h[0].wrapping_mul(5).wrapping_add(0x52dc_e729);

    k[1] = k[1].wrapping_mul(C64[1]);
    k[1] = k[1].rotate_left(33);
    k[1] = k[1].wrapping_mul(C64[0]);
    h[1] ^= k[1];
    h[1] = h[1].rotate_left(31);
    h[1] = h[1].wrapping_add(h[0]);
    h[1] = h[1].wrapping_mul(5).wrapping_add(0x3849_5ab5);
}

/// MurmurHash3 x64_128 (`murmur3f`).
#[derive(Clone)]
pub(crate) struct Murmur3f {
    h: [u64; 2],
    carry: [u64; 2],
    len: u32,
}

impl Murmur3f {
    pub(crate) fn new(seed: Option<i64>) -> Self {
        let s = seed.unwrap_or(0) as u64;
        Self {
            h: [s; 2],
            carry: [0; 2],
            len: 0,
        }
    }

    fn shift_in(&mut self, n: &mut usize, byte: u8) {
        let w = *n / 8;
        self.carry[w] = (self.carry[w] >> 8) | (u64::from(byte) << 56);
        if *n == 15 {
            let mut c = self.carry;
            block128x64(&mut self.h, &mut c);
            self.carry = c;
            *n = 0;
        } else {
            *n += 1;
        }
    }
}

impl Engine for Murmur3f {
    fn update(&mut self, data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u32);
        let mut n = (self.carry[1] & 15) as usize;
        let mut data = data;
        let i = (16 - n) & 15;
        if i != 0 && i <= data.len() {
            for &b in &data[..i] {
                self.shift_in(&mut n, b);
            }
            data = &data[i..];
        }
        // The carry registers double as the block variables (and keep the mixed values).
        while let Some((chunk, rest)) = data.split_first_chunk::<16>() {
            self.carry = load_le64(chunk);
            block128x64(&mut self.h, &mut self.carry);
            data = rest;
        }
        for &b in data {
            self.shift_in(&mut n, b);
        }
        self.carry[1] = (self.carry[1] & !0xff) | n as u64;
    }

    fn finish(self) -> Vec<u8> {
        let mut h = self.h;
        let n = (self.carry[1] & 15) as usize;
        let have1 = n.min(8);
        if have1 > 0 {
            let mut k = self.carry[0] >> (64 - 8 * have1 as u32);
            k = k.wrapping_mul(C64[0]);
            k = k.rotate_left(31);
            k = k.wrapping_mul(C64[1]);
            h[0] ^= k;
        }
        let have2 = n.saturating_sub(8);
        if have2 > 0 {
            let mut k = self.carry[1] >> (64 - 8 * have2 as u32);
            k = k.wrapping_mul(C64[1]);
            k = k.rotate_left(33);
            k = k.wrapping_mul(C64[0]);
            h[1] ^= k;
        }
        let len = u64::from(self.len);
        h[0] ^= len;
        h[1] ^= len;
        h[0] = h[0].wrapping_add(h[1]);
        h[1] = h[1].wrapping_add(h[0]);
        h[0] = fmix64(h[0]);
        h[1] = fmix64(h[1]);
        h[0] = h[0].wrapping_add(h[1]);
        h[1] = h[1].wrapping_add(h[0]);
        h.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u64s(&mut self.h)?;
        v.u64s(&mut self.carry)?;
        v.u32(&mut self.len)
    }
}
