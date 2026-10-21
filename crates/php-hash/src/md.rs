//! MD2, MD4 and MD5.

use crate::hasher::{Engine, Md32, Transform32, encode32, load_le32};
use crate::state::{StateResult, Visitor};

/// MD4 (`md4`).
#[derive(Clone)]
pub(crate) struct Md4(Md32<4>);

impl Md4 {
    pub(crate) fn new() -> Self {
        Self(Md32::new([
            0x6745_2301,
            0xefcd_ab89,
            0x98ba_dcfe,
            0x1032_5476,
        ]))
    }
}

fn md4_transform(state: &mut [u32; 4], block: &[u8; 64]) {
    let x = load_le32::<16>(block);
    let [mut a, mut b, mut c, mut d] = *state;
    let f = |x: u32, y: u32, z: u32| z ^ (x & (y ^ z));
    let g = |x: u32, y: u32, z: u32| (x & (y | z)) | (y & z);
    let h = |x: u32, y: u32, z: u32| x ^ y ^ z;

    // Round 1
    for i in 0..4 {
        let k = i * 4;
        a = a.wrapping_add(f(b, c, d)).wrapping_add(x[k]).rotate_left(3);
        d = d
            .wrapping_add(f(a, b, c))
            .wrapping_add(x[k + 1])
            .rotate_left(7);
        c = c
            .wrapping_add(f(d, a, b))
            .wrapping_add(x[k + 2])
            .rotate_left(11);
        b = b
            .wrapping_add(f(c, d, a))
            .wrapping_add(x[k + 3])
            .rotate_left(19);
    }
    // Round 2
    for k in 0..4 {
        a = a
            .wrapping_add(g(b, c, d))
            .wrapping_add(x[k])
            .wrapping_add(0x5A82_7999)
            .rotate_left(3);
        d = d
            .wrapping_add(g(a, b, c))
            .wrapping_add(x[k + 4])
            .wrapping_add(0x5A82_7999)
            .rotate_left(5);
        c = c
            .wrapping_add(g(d, a, b))
            .wrapping_add(x[k + 8])
            .wrapping_add(0x5A82_7999)
            .rotate_left(9);
        b = b
            .wrapping_add(g(c, d, a))
            .wrapping_add(x[k + 12])
            .wrapping_add(0x5A82_7999)
            .rotate_left(13);
    }
    // Round 3
    for k in [0usize, 2, 1, 3] {
        a = a
            .wrapping_add(h(b, c, d))
            .wrapping_add(x[k])
            .wrapping_add(0x6ED9_EBA1)
            .rotate_left(3);
        d = d
            .wrapping_add(h(a, b, c))
            .wrapping_add(x[k + 8])
            .wrapping_add(0x6ED9_EBA1)
            .rotate_left(9);
        c = c
            .wrapping_add(h(d, a, b))
            .wrapping_add(x[k + 4])
            .wrapping_add(0x6ED9_EBA1)
            .rotate_left(11);
        b = b
            .wrapping_add(h(c, d, a))
            .wrapping_add(x[k + 12])
            .wrapping_add(0x6ED9_EBA1)
            .rotate_left(15);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

impl Engine for Md4 {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data, md4_transform as Transform32<4>);
    }

    fn finish(mut self) -> Vec<u8> {
        self.0.pad(md4_transform, false);
        encode32(&self.0.state, false, 16)
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        self.0.visit(v)
    }
}

/// MD5 (`md5`); also backs the `md5()` helper.
#[derive(Clone)]
pub(crate) struct Md5 {
    state: [u32; 4],
    /// Bytes processed modulo 2^29 (`lo`) and the number of 2^29 wraps (`hi`).
    lo: u32,
    hi: u32,
    buffer: [u8; 64],
    /// The `block[16]` scratch words of the C context.  x86 builds never write
    /// them, so they stay zero unless a deserialised context carried other values.
    scratch: [u32; 16],
}

impl Md5 {
    pub(crate) fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            lo: 0,
            hi: 0,
            buffer: [0; 64],
            scratch: [0; 16],
        }
    }
}

const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

fn md5_transform(state: &mut [u32; 4], block: &[u8; 64]) {
    let x = load_le32::<16>(block);
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let t = a
            .wrapping_add(f)
            .wrapping_add(K[i])
            .wrapping_add(x[g])
            .rotate_left(SHIFTS[i]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(t);
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

impl Engine for Md5 {
    fn update(&mut self, data: &[u8]) {
        let saved_lo = self.lo;
        let size = data.len() as u64;
        self.lo = ((u64::from(saved_lo) + size) & 0x1fff_ffff) as u32;
        if self.lo < saved_lo {
            self.hi = self.hi.wrapping_add(1);
        }
        self.hi = self.hi.wrapping_add((size >> 29) as u32);
        let used = (saved_lo & 0x3f) as usize;
        let mut data = data;
        let state = &mut self.state;
        if used != 0 {
            let free = 64 - used;
            if data.len() < free {
                self.buffer[used..used + data.len()].copy_from_slice(data);
                return;
            }
            self.buffer[used..].copy_from_slice(&data[..free]);
            data = &data[free..];
            md5_transform(state, &self.buffer);
        }
        while let Some((block, rest)) = data.split_first_chunk::<64>() {
            md5_transform(state, block);
            data = rest;
        }
        self.buffer[..data.len()].copy_from_slice(data);
    }

    fn finish(mut self) -> Vec<u8> {
        let mut used = (self.lo & 0x3f) as usize;
        self.buffer[used] = 0x80;
        used += 1;
        let free = 64 - used;
        if free < 8 {
            self.buffer[used..].fill(0);
            md5_transform(&mut self.state, &self.buffer);
            used = 0;
        }
        let free = 64 - used;
        self.buffer[used..used + free - 8].fill(0);
        self.buffer[56..60].copy_from_slice(&(self.lo << 3).to_le_bytes());
        self.buffer[60..64].copy_from_slice(&self.hi.to_le_bytes());
        md5_transform(&mut self.state, &self.buffer);
        encode32(&self.state, false, 16)
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32(&mut self.lo)?;
        v.u32(&mut self.hi)?;
        v.u32s(&mut self.state)?;
        v.bytes(&mut self.buffer)?;
        v.u32s(&mut self.scratch)
    }
}

/// MD2 (`md2`).
#[derive(Clone)]
pub(crate) struct Md2 {
    state: [u8; 48],
    checksum: [u8; 16],
    buffer: [u8; 16],
    in_buffer: u8,
}

impl Md2 {
    pub(crate) fn new() -> Self {
        Self {
            state: [0; 48],
            checksum: [0; 16],
            buffer: [0; 16],
            in_buffer: 0,
        }
    }

    fn transform(&mut self, block: &[u8; 16]) {
        for (i, &b) in block.iter().enumerate() {
            self.state[16 + i] = b;
            self.state[32 + i] = b ^ self.state[i];
        }
        let mut t = 0u8;
        for round in 0..18u8 {
            for s in &mut self.state {
                *s ^= MD2_S[usize::from(t)];
                t = *s;
            }
            t = t.wrapping_add(round);
        }
        // Update checksum -- must be after transform to avoid fouling up last message block.
        t = self.checksum[15];
        for (c, &b) in self.checksum.iter_mut().zip(block) {
            *c ^= MD2_S[usize::from(b ^ t)];
            t = *c;
        }
    }
}

impl Engine for Md2 {
    fn update(&mut self, data: &[u8]) {
        let mut p = data;
        let in_buf = usize::from(self.in_buffer);
        if in_buf != 0 {
            if in_buf + p.len() < 16 {
                self.buffer[in_buf..in_buf + p.len()].copy_from_slice(p);
                self.in_buffer = (in_buf + p.len()) as u8;
                return;
            }
            let need = 16 - in_buf;
            self.buffer[in_buf..].copy_from_slice(&p[..need]);
            let block = self.buffer;
            self.transform(&block);
            p = &p[need..];
            self.in_buffer = 0;
        }
        while let Some((block, rest)) = p.split_first_chunk::<16>() {
            self.transform(block);
            p = rest;
        }
        if !p.is_empty() {
            self.buffer[..p.len()].copy_from_slice(p);
            self.in_buffer = p.len() as u8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let used = usize::from(self.in_buffer);
        let pad = (16 - used) as u8;
        self.buffer[used..].fill(pad);
        let block = self.buffer;
        self.transform(&block);
        let checksum = self.checksum;
        self.transform(&checksum);
        self.state[..16].to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.bytes(&mut self.state)?;
        v.bytes(&mut self.checksum)?;
        v.bytes(&mut self.buffer)?;
        v.u8(&mut self.in_buffer)
    }

    fn validate(&self) -> bool {
        usize::from(self.in_buffer) < self.buffer.len()
    }
}

const MD2_S: [u8; 256] = [
    41, 46, 67, 201, 162, 216, 124, 1, 61, 54, 84, 161, 236, 240, 6, 19, 98, 167, 5, 243, 192, 199,
    115, 140, 152, 147, 43, 217, 188, 76, 130, 202, 30, 155, 87, 60, 253, 212, 224, 22, 103, 66,
    111, 24, 138, 23, 229, 18, 190, 78, 196, 214, 218, 158, 222, 73, 160, 251, 245, 142, 187, 47,
    238, 122, 169, 104, 121, 145, 21, 178, 7, 63, 148, 194, 16, 137, 11, 34, 95, 33, 128, 127, 93,
    154, 90, 144, 50, 39, 53, 62, 204, 231, 191, 247, 151, 3, 255, 25, 48, 179, 72, 165, 181, 209,
    215, 94, 146, 42, 172, 86, 170, 198, 79, 184, 56, 210, 150, 164, 125, 182, 118, 252, 107, 226,
    156, 116, 4, 241, 69, 157, 112, 89, 100, 113, 135, 32, 134, 91, 207, 101, 230, 45, 168, 2, 27,
    96, 37, 173, 174, 176, 185, 246, 28, 70, 97, 105, 52, 64, 126, 15, 85, 71, 163, 35, 221, 81,
    175, 58, 195, 92, 249, 206, 186, 197, 234, 38, 44, 83, 13, 110, 133, 40, 132, 9, 211, 223, 205,
    244, 65, 129, 77, 82, 106, 220, 55, 200, 108, 193, 171, 250, 36, 225, 123, 8, 12, 189, 177, 74,
    120, 136, 149, 139, 227, 99, 232, 109, 233, 203, 213, 254, 59, 0, 29, 57, 242, 239, 183, 14,
    102, 88, 208, 228, 166, 119, 114, 248, 235, 117, 75, 10, 49, 68, 80, 180, 143, 237, 31, 26,
    219, 153, 141, 51, 159, 17, 131, 20,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];
