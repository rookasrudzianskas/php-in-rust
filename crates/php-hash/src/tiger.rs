//! Tiger (3 and 4 passes, 128/160/192-bit digests).

use crate::hasher::{Engine, load_le64};
use crate::state::{StateResult, Visitor};
use crate::tables::tiger::TABLE;

fn t(n: usize, idx: u64) -> u64 {
    TABLE[n * 256 + (idx & 0xff) as usize]
}

fn round(a: &mut u64, b: &mut u64, c: &mut u64, x: u64, mul: u64) {
    *c ^= x;
    let cc = *c;
    *a = a.wrapping_sub(t(0, cc) ^ t(1, cc >> 16) ^ t(2, cc >> 32) ^ t(3, cc >> 48));
    *b = b
        .wrapping_add(t(3, cc >> 8) ^ t(2, cc >> 24) ^ t(1, cc >> 40) ^ t(0, cc >> 56))
        .wrapping_mul(mul);
}

fn pass(a: &mut u64, b: &mut u64, c: &mut u64, x: &[u64; 8], mul: u64) {
    round(a, b, c, x[0], mul);
    round(b, c, a, x[1], mul);
    round(c, a, b, x[2], mul);
    round(a, b, c, x[3], mul);
    round(b, c, a, x[4], mul);
    round(c, a, b, x[5], mul);
    round(a, b, c, x[6], mul);
    round(b, c, a, x[7], mul);
}

fn key_schedule(x: &mut [u64; 8]) {
    x[0] = x[0].wrapping_sub(x[7] ^ 0xA5A5_A5A5_A5A5_A5A5);
    x[1] ^= x[0];
    x[2] = x[2].wrapping_add(x[1]);
    x[3] = x[3].wrapping_sub(x[2] ^ ((!x[1]) << 19));
    x[4] ^= x[3];
    x[5] = x[5].wrapping_add(x[4]);
    x[6] = x[6].wrapping_sub(x[5] ^ ((!x[4]) >> 23));
    x[7] ^= x[6];
    x[0] = x[0].wrapping_add(x[7]);
    x[1] = x[1].wrapping_sub(x[0] ^ ((!x[7]) << 19));
    x[2] ^= x[1];
    x[3] = x[3].wrapping_add(x[2]);
    x[4] = x[4].wrapping_sub(x[3] ^ ((!x[2]) >> 23));
    x[5] ^= x[4];
    x[6] = x[6].wrapping_add(x[5]);
    x[7] = x[7].wrapping_sub(x[6] ^ 0x0123_4567_89AB_CDEF);
}

fn compress(extra_passes: u32, block: &[u8; 64], state: &mut [u64; 3]) {
    let mut x = load_le64::<8>(block);
    let (mut a, mut b, mut c) = (state[0], state[1], state[2]);
    let (aa, bb, cc) = (a, b, c);
    pass(&mut a, &mut b, &mut c, &x, 5);
    key_schedule(&mut x);
    pass(&mut c, &mut a, &mut b, &x, 7);
    key_schedule(&mut x);
    pass(&mut b, &mut c, &mut a, &x, 9);
    for _ in 0..extra_passes {
        key_schedule(&mut x);
        pass(&mut a, &mut b, &mut c, &x, 9);
        let tmp = a;
        a = c;
        c = b;
        b = tmp;
    }
    state[0] = a ^ aa;
    state[1] = b.wrapping_sub(bb);
    state[2] = c.wrapping_add(cc);
}

/// Tiger engine; the layout mirrors `PHP_TIGER_CTX`.
#[derive(Clone)]
pub(crate) struct Tiger {
    state: [u64; 3],
    passed: u64,
    buffer: [u8; 64],
    length: u32,
    /// Passes beyond the mandatory three (0 for `tiger*,3`, 1 for `tiger*,4`).
    extra_passes: u32,
    digest_len: usize,
}

impl Tiger {
    pub(crate) fn new(passes: u32, digest_bits: usize) -> Self {
        Self {
            state: [
                0x0123_4567_89AB_CDEF,
                0xFEDC_BA98_7654_3210,
                0xF096_A5B4_C3B2_E187,
            ],
            passed: 0,
            buffer: [0; 64],
            length: 0,
            extra_passes: passes - 3,
            digest_len: digest_bits / 8,
        }
    }

    fn compress_buffer(&mut self) {
        let buffer = self.buffer;
        compress(self.extra_passes, &buffer, &mut self.state);
    }
}

impl Engine for Tiger {
    fn update(&mut self, input: &[u8]) {
        let len = input.len();
        let length = self.length as usize;
        if length + len < 64 {
            self.buffer[length..length + len].copy_from_slice(input);
            self.length += len as u32;
            return;
        }
        let r = (length + len) % 64;
        let mut i = 0;
        if length != 0 {
            i = 64 - length;
            self.buffer[length..].copy_from_slice(&input[..i]);
            self.compress_buffer();
            self.buffer = [0; 64];
            self.passed = self.passed.wrapping_add(512);
        }
        while i + 64 <= len {
            self.buffer.copy_from_slice(&input[i..i + 64]);
            self.compress_buffer();
            self.passed = self.passed.wrapping_add(512);
            i += 64;
        }
        self.buffer[r..].fill(0);
        self.buffer[..r].copy_from_slice(&input[i..i + r]);
        self.length = r as u32;
    }

    fn finish(mut self) -> Vec<u8> {
        self.passed = self.passed.wrapping_add(u64::from(self.length) << 3);
        let mut length = self.length as usize;
        self.buffer[length] = 0x1;
        length += 1;
        if length & 7 != 0 {
            let pad = 8 - length % 8;
            self.buffer[length..length + pad].fill(0);
            length += pad;
        }
        if length > 56 {
            self.buffer[length..].fill(0);
            self.compress_buffer();
            self.buffer[..56].fill(0);
        } else {
            self.buffer[length..56].fill(0);
        }
        let passed = self.passed;
        self.buffer[56..].copy_from_slice(&passed.to_le_bytes());
        self.compress_buffer();
        let mut out: Vec<u8> = self.state.iter().flat_map(|w| w.to_le_bytes()).collect();
        out.truncate(self.digest_len);
        out
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u64s(&mut self.state)?;
        v.u64(&mut self.passed)?;
        v.bytes(&mut self.buffer)?;
        v.u32(&mut self.length)
    }

    fn validate(&self) -> bool {
        self.length < 64
    }
}
