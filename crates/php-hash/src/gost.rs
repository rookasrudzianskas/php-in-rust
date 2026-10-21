//! GOST R 34.11-94 (`gost` with the test S-boxes, `gost-crypto` with CryptoPro).

use std::cmp::Ordering;

use crate::hasher::{Engine, load_le32};
use crate::state::{StateResult, Visitor};
use crate::tables::gost::{TABLES_CRYPTO, TABLES_TEST};

// The wide permutations below are transcribed from the macros of `hash_gost.c`.
#[rustfmt::skip]
mod mix {
pub(super) fn shift12(u: &mut [u32; 8], m: &[u32; 8], s: &[u32; 8]) {
    u[0] = m[0] ^ s[6];
    u[1] = m[1] ^ s[7];
    u[2] = m[2] ^ (s[0] << 16) ^ (s[0] >> 16) ^ (s[0] & 0xffff) ^ (s[1] & 0xffff) ^ (s[1] >> 16) ^ (s[2] << 16) ^ s[6] ^ (s[6] << 16) ^ (s[7] & 0xffff0000) ^ (s[7] >> 16);
    u[3] = m[3] ^ (s[0] & 0xffff) ^ (s[0] << 16) ^ (s[1] & 0xffff) ^ (s[1] << 16) ^ (s[1] >> 16) ^ (s[2] << 16) ^ (s[2] >> 16) ^ (s[3] << 16) ^ s[6] ^ (s[6] << 16) ^ (s[6] >> 16) ^ (s[7] & 0xffff) ^ (s[7] << 16) ^ (s[7] >> 16);
    u[4] = m[4] ^ (s[0] & 0xffff0000) ^ (s[0] << 16) ^ (s[0] >> 16) ^ (s[1] & 0xffff0000) ^ (s[1] >> 16) ^ (s[2] << 16) ^ (s[2] >> 16) ^ (s[3] << 16) ^ (s[3] >> 16) ^ (s[4] << 16) ^ (s[6] << 16) ^ (s[6] >> 16) ^(s[7] & 0xffff) ^ (s[7] << 16) ^ (s[7] >> 16);
    u[5] = m[5] ^ (s[0] << 16) ^ (s[0] >> 16) ^ (s[0] & 0xffff0000) ^ (s[1] & 0xffff) ^ s[2] ^ (s[2] >> 16) ^ (s[3] << 16) ^ (s[3] >> 16) ^ (s[4] << 16) ^ (s[4] >> 16) ^ (s[5] << 16) ^ (s[6] << 16) ^ (s[6] >> 16) ^ (s[7] & 0xffff0000) ^ (s[7] << 16) ^ (s[7] >> 16);
    u[6] = m[6] ^ s[0] ^ (s[1] >> 16) ^ (s[2] << 16) ^ s[3] ^ (s[3] >> 16) ^ (s[4] << 16) ^ (s[4] >> 16) ^ (s[5] << 16) ^ (s[5] >> 16) ^ s[6] ^ (s[6] << 16) ^ (s[6] >> 16) ^ (s[7] << 16);
    u[7] = m[7] ^ (s[0] & 0xffff0000) ^ (s[0] << 16) ^ (s[1] & 0xffff) ^ (s[1] << 16) ^ (s[2] >> 16) ^ (s[3] << 16) ^ s[4] ^ (s[4] >> 16) ^ (s[5] << 16) ^ (s[5] >> 16) ^ (s[6] >> 16) ^ (s[7] & 0xffff) ^ (s[7] << 16) ^ (s[7] >> 16);
}

pub(super) fn shift16(h: &[u32; 8], v: &mut [u32; 8], u: &[u32; 8]) {
    v[0] = h[0] ^ (u[1] << 16) ^ (u[0] >> 16);
    v[1] = h[1] ^ (u[2] << 16) ^ (u[1] >> 16);
    v[2] = h[2] ^ (u[3] << 16) ^ (u[2] >> 16);
    v[3] = h[3] ^ (u[4] << 16) ^ (u[3] >> 16);
    v[4] = h[4] ^ (u[5] << 16) ^ (u[4] >> 16);
    v[5] = h[5] ^ (u[6] << 16) ^ (u[5] >> 16);
    v[6] = h[6] ^ (u[7] << 16) ^ (u[6] >> 16);
    v[7] = h[7] ^ (u[0] & 0xffff0000) ^ (u[0] << 16) ^ (u[7] >> 16) ^ (u[1] & 0xffff0000) ^ (u[1] << 16) ^ (u[6] << 16) ^ (u[7] & 0xffff0000);
}

pub(super) fn shift61(h: &mut [u32; 8], v: &[u32; 8]) {
    h[0] = (v[0] & 0xffff0000) ^ (v[0] << 16) ^ (v[0] >> 16) ^ (v[1] >> 16) ^ (v[1] & 0xffff0000) ^ (v[2] << 16) ^ (v[3] >> 16) ^ (v[4] << 16) ^ (v[5] >> 16) ^ v[5] ^ (v[6] >> 16) ^ (v[7] << 16) ^ (v[7] >> 16) ^ (v[7] & 0xffff);
    h[1] = (v[0] << 16) ^ (v[0] >> 16) ^ (v[0] & 0xffff0000) ^ (v[1] & 0xffff) ^ v[2] ^ (v[2] >> 16) ^ (v[3] << 16) ^ (v[4] >> 16) ^ (v[5] << 16) ^ (v[6] << 16) ^ v[6] ^ (v[7] & 0xffff0000) ^ (v[7] >> 16);
    h[2] = (v[0] & 0xffff) ^ (v[0] << 16) ^ (v[1] << 16) ^ (v[1] >> 16) ^ (v[1] & 0xffff0000) ^ (v[2] << 16) ^ (v[3] >> 16) ^ v[3] ^ (v[4] << 16) ^ (v[5] >> 16) ^ v[6] ^ (v[6] >> 16) ^ (v[7] & 0xffff) ^ (v[7] << 16) ^ (v[7] >> 16);
    h[3] = (v[0] << 16) ^ (v[0] >> 16) ^ (v[0] & 0xffff0000) ^ (v[1] & 0xffff0000) ^ (v[1] >> 16) ^ (v[2] << 16) ^ (v[2] >> 16) ^ v[2] ^ (v[3] << 16) ^ (v[4] >> 16) ^ v[4] ^ (v[5] << 16) ^ (v[6] << 16) ^ (v[7] & 0xffff) ^ (v[7] >> 16);
    h[4] = (v[0] >> 16) ^ (v[1] << 16) ^ v[1] ^ (v[2] >> 16) ^ v[2] ^ (v[3] << 16) ^ (v[3] >> 16) ^ v[3] ^ (v[4] << 16) ^ (v[5] >> 16) ^ v[5] ^ (v[6] << 16) ^ (v[6] >> 16) ^ (v[7] << 16);
    h[5] = (v[0] << 16) ^ (v[0] & 0xffff0000) ^ (v[1] << 16) ^ (v[1] >> 16) ^ (v[1] & 0xffff0000) ^ (v[2] << 16) ^ v[2] ^ (v[3] >> 16) ^ v[3] ^ (v[4] << 16) ^ (v[4] >> 16) ^ v[4] ^ (v[5] << 16) ^ (v[6] << 16) ^ (v[6] >> 16) ^ v[6] ^ (v[7] << 16) ^ (v[7] >> 16) ^ (v[7] & 0xffff0000);
    h[6] = v[0] ^ v[2] ^ (v[2] >> 16) ^ v[3] ^ (v[3] << 16) ^ v[4] ^ (v[4] >> 16) ^ (v[5] << 16) ^ (v[5] >> 16) ^ v[5] ^ (v[6] << 16) ^ (v[6] >> 16) ^ v[6] ^ (v[7] << 16) ^ v[7];
    h[7] = v[0] ^ (v[0] >> 16) ^ (v[1] << 16) ^ (v[1] >> 16) ^ (v[2] << 16) ^ (v[3] >> 16) ^ v[3] ^ (v[4] << 16) ^ v[4] ^ (v[5] >> 16) ^ v[5] ^ (v[6] << 16) ^ (v[6] >> 16) ^ (v[7] << 16) ^ v[7];
}

pub(super) fn permute_p(key: &mut [u32; 8], w: &[u32; 8]) {
    key[0] = (w[0] & 0x000000ff) | ((w[2] & 0x000000ff) << 8) | ((w[4] & 0x000000ff) << 16) | ((w[6] & 0x000000ff) << 24);
    key[1] = ((w[0] & 0x0000ff00) >> 8) | (w[2] & 0x0000ff00) | ((w[4] & 0x0000ff00) << 8) | ((w[6] & 0x0000ff00) << 16);
    key[2] = ((w[0] & 0x00ff0000) >> 16) | ((w[2] & 0x00ff0000) >> 8) | (w[4] & 0x00ff0000) | ((w[6] & 0x00ff0000) << 8);
    key[3] = ((w[0] & 0xff000000) >> 24) | ((w[2] & 0xff000000) >> 16) | ((w[4] & 0xff000000) >> 8) | (w[6] & 0xff000000);
    key[4] = (w[1] & 0x000000ff) | ((w[3] & 0x000000ff) << 8) | ((w[5] & 0x000000ff) << 16) | ((w[7] & 0x000000ff) << 24);
    key[5] = ((w[1] & 0x0000ff00) >> 8) | (w[3] & 0x0000ff00) | ((w[5] & 0x0000ff00) << 8) | ((w[7] & 0x0000ff00) << 16);
    key[6] = ((w[1] & 0x00ff0000) >> 16) | ((w[3] & 0x00ff0000) >> 8) | (w[5] & 0x00ff0000) | ((w[7] & 0x00ff0000) << 8);
    key[7] = ((w[1] & 0xff000000) >> 24) | ((w[3] & 0xff000000) >> 16) | ((w[5] & 0xff000000) >> 8) | (w[7] & 0xff000000);
}

}

use mix::{permute_p, shift12, shift16, shift61};

const MAX32: u32 = 0xffff_ffff;

/// GOST engine; the layout mirrors `PHP_GOST_CTX`.
#[derive(Clone)]
pub(crate) struct Gost {
    state: [u32; 16],
    count: [u32; 2],
    length: u8,
    buffer: [u8; 32],
    crypto: bool,
}

impl Gost {
    pub(crate) fn new(crypto: bool) -> Self {
        Self {
            state: [0; 16],
            count: [0; 2],
            length: 0,
            buffer: [0; 32],
            crypto,
        }
    }

    fn tables(&self) -> &'static [u32; 1024] {
        if self.crypto {
            &TABLES_CRYPTO
        } else {
            &TABLES_TEST
        }
    }

    /// The compression function `Gost()` working on `state[0..8]`.
    fn compress(&mut self, data: &[u32; 8]) {
        let tables = self.tables();
        let sub = |t: u32| -> u32 {
            tables[(t & 0xff) as usize]
                ^ tables[256 + ((t >> 8) & 0xff) as usize]
                ^ tables[512 + ((t >> 16) & 0xff) as usize]
                ^ tables[768 + (t >> 24) as usize]
        };
        let mut h = [0u32; 8];
        h.copy_from_slice(&self.state[..8]);
        let mut u = h;
        let mut v = *data;
        let mut s = [0u32; 8];
        let mut key = [0u32; 8];
        let mut w = [0u32; 8];
        for i in (0..8).step_by(2) {
            for k in 0..8 {
                w[k] = u[k] ^ v[k];
            }
            permute_p(&mut key, &w);
            let mut r = h[i];
            let mut l = h[i + 1];
            let order: [(usize, usize); 16] = [
                (0, 1),
                (2, 3),
                (4, 5),
                (6, 7),
                (0, 1),
                (2, 3),
                (4, 5),
                (6, 7),
                (0, 1),
                (2, 3),
                (4, 5),
                (6, 7),
                (7, 6),
                (5, 4),
                (3, 2),
                (1, 0),
            ];
            for (k1, k2) in order {
                l ^= sub(key[k1].wrapping_add(r));
                r ^= sub(key[k2].wrapping_add(l));
            }
            std::mem::swap(&mut l, &mut r);
            s[i] = r;
            s[i + 1] = l;
            if i != 6 {
                // A(u)
                let l = u[0] ^ u[2];
                let r = u[1] ^ u[3];
                u[0] = u[2];
                u[1] = u[3];
                u[2] = u[4];
                u[3] = u[5];
                u[4] = u[6];
                u[5] = u[7];
                u[6] = l;
                u[7] = r;
                if i == 2 {
                    // C(u)
                    u[0] ^= 0xff00_ff00;
                    u[1] ^= 0xff00_ff00;
                    u[2] ^= 0x00ff_00ff;
                    u[3] ^= 0x00ff_00ff;
                    u[4] ^= 0x00ff_ff00;
                    u[5] ^= 0xff00_00ff;
                    u[6] ^= 0x0000_00ff;
                    u[7] ^= 0xff00_ffff;
                }
                // AA(v)
                let l = v[0];
                let r = v[2];
                v[0] = v[4];
                v[2] = v[6];
                v[4] = l ^ r;
                v[6] = v[0] ^ r;
                let l = v[1];
                let r = v[3];
                v[1] = v[5];
                v[3] = v[7];
                v[5] = l ^ r;
                v[7] = v[1] ^ r;
            }
        }
        shift12(&mut u, data, &s);
        shift16(&h, &mut v, &u);
        let mut new_h = h;
        shift61(&mut new_h, &v);
        self.state[..8].copy_from_slice(&new_h);
    }

    fn transform(&mut self, input: &[u8; 32]) {
        let data = load_le32::<8>(input);
        let mut temp = 0u32;
        for (sum, &word) in self.state[8..].iter_mut().zip(&data) {
            *sum = sum.wrapping_add(word.wrapping_add(temp));
            temp = match (*sum).cmp(&word) {
                Ordering::Less => 1,
                Ordering::Equal => temp,
                Ordering::Greater => 0,
            };
        }
        self.compress(&data);
    }
}

impl Engine for Gost {
    fn update(&mut self, input: &[u8]) {
        let len = input.len();
        let bits = (len as u64).wrapping_mul(8);
        if u64::from(MAX32 - self.count[0]) < bits {
            self.count[1] = self.count[1].wrapping_add(1);
            self.count[0] = MAX32 - self.count[0];
            self.count[0] = bits.wrapping_sub(u64::from(self.count[0])) as u32;
        } else {
            self.count[0] = (u64::from(self.count[0]).wrapping_add(bits)) as u32;
        }
        let length = usize::from(self.length);
        if length + len < 32 {
            self.buffer[length..length + len].copy_from_slice(input);
            self.length += len as u8;
            return;
        }
        let r = (length + len) % 32;
        let mut i = 0;
        if length != 0 {
            i = 32 - length;
            self.buffer[length..].copy_from_slice(&input[..i]);
            let buffer = self.buffer;
            self.transform(&buffer);
        }
        while i + 32 <= len {
            let block: [u8; 32] = input[i..i + 32].try_into().expect("block length");
            self.transform(&block);
            i += 32;
        }
        self.buffer[..r].copy_from_slice(&input[i..i + r]);
        self.buffer[r..].fill(0);
        self.length = r as u8;
    }

    fn finish(mut self) -> Vec<u8> {
        if self.length != 0 {
            let buffer = self.buffer;
            self.transform(&buffer);
        }
        let mut l = [0u32; 8];
        l[0] = self.count[0];
        l[1] = self.count[1];
        self.compress(&l);
        l.copy_from_slice(&self.state[8..]);
        self.compress(&l);
        self.state[..8]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32s(&mut self.state)?;
        v.u32s(&mut self.count)?;
        v.u8(&mut self.length)?;
        v.bytes(&mut self.buffer)
    }

    fn validate(&self) -> bool {
        usize::from(self.length) < self.buffer.len()
    }
}
