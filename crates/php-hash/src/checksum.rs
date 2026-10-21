//! Small checksum-style hashes: adler32, crc32 (three flavours), fnv and joaat.

use crate::hasher::Engine;
use crate::state::{StateResult, Visitor};

/// Adler-32 (`adler32`).
#[derive(Clone)]
pub(crate) struct Adler32 {
    state: u32,
}

impl Adler32 {
    pub(crate) fn new() -> Self {
        Self { state: 1 }
    }
}

impl Engine for Adler32 {
    fn update(&mut self, data: &[u8]) {
        let mut s0 = self.state & 0xffff;
        let mut s1 = (self.state >> 16) & 0xffff;
        for &b in data {
            s0 += u32::from(b);
            s1 += s0;
            if s1 >= 0x7fff_ffff {
                s0 %= 65521;
                s1 %= 65521;
            }
        }
        s0 %= 65521;
        s1 %= 65521;
        self.state = s0 + (s1 << 16);
    }

    fn finish(self) -> Vec<u8> {
        self.state.to_be_bytes().to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32(&mut self.state)
    }
}

/// The three CRC-32 flavours of ext/hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CrcKind {
    /// `crc32`: the bzip2 CRC (MSB first, polynomial 0x04C11DB7), little-endian digest.
    Bzip2,
    /// `crc32b`: the ubiquitous zlib/PNG/Ethernet CRC, big-endian digest.
    Iso,
    /// `crc32c`: Castagnoli (polynomial 0x1EDC6F41), big-endian digest.
    Castagnoli,
}

const fn msb_table(poly: u32) -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ poly
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn lsb_table(poly: u32) -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ poly
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = msb_table(0x04c1_1db7);
static CRC32B_TABLE: [u32; 256] = lsb_table(0xedb8_8320);
static CRC32C_TABLE: [u32; 256] = lsb_table(0x82f6_3b78);

/// CRC-32 engine (`crc32`, `crc32b`, `crc32c`).
#[derive(Clone)]
pub(crate) struct Crc32 {
    state: u32,
    kind: CrcKind,
}

impl Crc32 {
    pub(crate) fn new(kind: CrcKind) -> Self {
        Self { state: !0, kind }
    }
}

impl Engine for Crc32 {
    fn update(&mut self, data: &[u8]) {
        let mut state = self.state;
        match self.kind {
            CrcKind::Bzip2 => {
                for &b in data {
                    state = (state << 8) ^ CRC32_TABLE[((state >> 24) as u8 ^ b) as usize];
                }
            }
            CrcKind::Iso => {
                for &b in data {
                    state = (state >> 8) ^ CRC32B_TABLE[((state as u8) ^ b) as usize];
                }
            }
            CrcKind::Castagnoli => {
                for &b in data {
                    state = (state >> 8) ^ CRC32C_TABLE[((state as u8) ^ b) as usize];
                }
            }
        }
        self.state = state;
    }

    fn finish(self) -> Vec<u8> {
        let value = !self.state;
        match self.kind {
            CrcKind::Bzip2 => value.to_le_bytes().to_vec(),
            _ => value.to_be_bytes().to_vec(),
        }
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32(&mut self.state)
    }
}

/// CRC-32 (`crc32b`) of `data` as returned by PHP's `crc32()` function.
pub fn crc32(data: &[u8]) -> u32 {
    let mut state = !0u32;
    for &b in data {
        state = (state >> 8) ^ CRC32B_TABLE[((state as u8) ^ b) as usize];
    }
    !state
}

const FNV_32_PRIME: u32 = 0x0100_0193;
const FNV_64_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 32-bit Fowler/Noll/Vo (`fnv132`, `fnv1a32`).
#[derive(Clone)]
pub(crate) struct Fnv32 {
    state: u32,
    /// FNV-1a (xor then multiply) instead of FNV-1.
    alternate: bool,
}

impl Fnv32 {
    pub(crate) fn new(alternate: bool) -> Self {
        Self {
            state: 0x811c_9dc5,
            alternate,
        }
    }
}

impl Engine for Fnv32 {
    fn update(&mut self, data: &[u8]) {
        let mut h = self.state;
        if self.alternate {
            for &b in data {
                h ^= u32::from(b);
                h = h.wrapping_mul(FNV_32_PRIME);
            }
        } else {
            for &b in data {
                h = h.wrapping_mul(FNV_32_PRIME);
                h ^= u32::from(b);
            }
        }
        self.state = h;
    }

    fn finish(self) -> Vec<u8> {
        self.state.to_be_bytes().to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32(&mut self.state)
    }
}

/// 64-bit Fowler/Noll/Vo (`fnv164`, `fnv1a64`).
#[derive(Clone)]
pub(crate) struct Fnv64 {
    state: u64,
    alternate: bool,
}

impl Fnv64 {
    pub(crate) fn new(alternate: bool) -> Self {
        Self {
            state: 0xcbf2_9ce4_8422_2325,
            alternate,
        }
    }
}

impl Engine for Fnv64 {
    fn update(&mut self, data: &[u8]) {
        let mut h = self.state;
        if self.alternate {
            for &b in data {
                h ^= u64::from(b);
                h = h.wrapping_mul(FNV_64_PRIME);
            }
        } else {
            for &b in data {
                h = h.wrapping_mul(FNV_64_PRIME);
                h ^= u64::from(b);
            }
        }
        self.state = h;
    }

    fn finish(self) -> Vec<u8> {
        self.state.to_be_bytes().to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u64(&mut self.state)
    }
}

/// Jenkins one-at-a-time (`joaat`).
#[derive(Clone)]
pub(crate) struct Joaat {
    state: u32,
}

impl Joaat {
    pub(crate) fn new() -> Self {
        Self { state: 0 }
    }
}

impl Engine for Joaat {
    fn update(&mut self, data: &[u8]) {
        let mut h = self.state;
        for &b in data {
            h = h.wrapping_add(u32::from(b));
            h = h.wrapping_add(h << 10);
            h ^= h >> 6;
        }
        self.state = h;
    }

    fn finish(self) -> Vec<u8> {
        let mut h = self.state;
        h = h.wrapping_add(h << 3);
        h ^= h >> 11;
        h = h.wrapping_add(h << 15);
        h.to_be_bytes().to_vec()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32(&mut self.state)
    }
}
