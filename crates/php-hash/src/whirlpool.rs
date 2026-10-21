//! Whirlpool.

use crate::hasher::{Engine, load_be64};
use crate::state::{StateResult, Visitor};
use crate::tables::whirlpool::{C0, RC};

const ROUNDS: usize = 10;

fn lookup(table: usize, byte: u64) -> u64 {
    C0[(byte & 0xff) as usize].rotate_right(8 * table as u32)
}

/// One application of the (W) round function: `L[i] = XOR_j Cj[byte j of k[(i - j) mod 8]]`.
fn round_function(k: &[u64; 8]) -> [u64; 8] {
    let mut l = [0u64; 8];
    for (i, out) in l.iter_mut().enumerate() {
        let mut acc = 0u64;
        for j in 0..8 {
            acc ^= lookup(j, k[(i + 8 - j) & 7] >> (56 - 8 * j));
        }
        *out = acc;
    }
    l
}

/// Whirlpool (`whirlpool`); the layout mirrors `PHP_WHIRLPOOL_CTX`.
#[derive(Clone)]
pub(crate) struct Whirlpool {
    state: [u64; 8],
    /// 256-bit big-endian message length in bits.
    bitlength: [u8; 32],
    pos: u32,
    bits: u32,
    data: [u8; 64],
}

impl Whirlpool {
    pub(crate) fn new() -> Self {
        Self {
            state: [0; 8],
            bitlength: [0; 32],
            pos: 0,
            bits: 0,
            data: [0; 64],
        }
    }

    fn transform(&mut self) {
        let block = load_be64::<8>(&self.data);
        let mut k = self.state;
        let mut state = [0u64; 8];
        for i in 0..8 {
            state[i] = block[i] ^ k[i];
        }
        for rc in RC.iter().skip(1).take(ROUNDS) {
            // Compute K^r from K^{r-1}.
            let mut l = round_function(&k);
            l[0] ^= rc;
            k = l;
            // Apply the r-th round transformation to the cipher state.
            let mut l = round_function(&state);
            for i in 0..8 {
                l[i] ^= k[i];
            }
            state = l;
        }
        // Miyaguchi-Preneel compression.
        for i in 0..8 {
            self.state[i] ^= state[i] ^ block[i];
        }
    }
}

impl Whirlpool {
    /// Appends one byte to a buffer holding `rem` (1..=7) extra bits.
    fn push_unaligned(&mut self, b: u8, rem: u32) {
        self.data[self.pos as usize] |= b >> rem;
        self.pos += 1;
        self.bits += 8 - rem;
        if self.bits == 512 {
            self.transform();
            self.bits = 0;
            self.pos = 0;
        }
        self.data[self.pos as usize] = b << (8 - rem);
        self.bits += rem;
    }
}

impl Engine for Whirlpool {
    fn update(&mut self, data: &[u8]) {
        // Tally the length of the added data (256-bit big-endian addition).
        let mut value = (data.len() as u64).wrapping_mul(8);
        let mut carry = 0u32;
        let mut i = 32usize;
        while i > 0 && (carry != 0 || value != 0) {
            i -= 1;
            carry += u32::from(self.bitlength[i]) + (value as u32 & 0xff);
            self.bitlength[i] = carry as u8;
            carry >>= 8;
            value >>= 8;
        }
        let rem = self.bits & 7;
        if rem != 0 {
            // A deserialised context may hold a partial byte; PHP's bit-level
            // code then shifts every input byte across the byte boundary.
            for &b in data {
                self.push_unaligned(b, rem);
            }
            return;
        }
        // Byte aligned: bytes are appended one position at a time; the slot
        // after the last written byte is always cleared (visible in serialised
        // contexts).  Only the very first byte is ORed into its slot like in C
        // (`buffer[pos] |= b`); the slot is zero unless the state was crafted.
        let Some(&first) = data.first() else { return };
        let first = first | self.data[self.pos as usize];
        let mut rest = data;
        let mut is_first_chunk = true;
        while !rest.is_empty() {
            let pos = self.pos as usize;
            let n = rest.len().min(64 - pos);
            self.data[pos..pos + n].copy_from_slice(&rest[..n]);
            if is_first_chunk {
                self.data[pos] = first;
                is_first_chunk = false;
            }
            self.pos += n as u32;
            self.bits += 8 * n as u32;
            rest = &rest[n..];
            if self.bits == 512 {
                self.transform();
                self.bits = 0;
                self.pos = 0;
            }
            self.data[self.pos as usize] = 0;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let mut pos = self.pos as usize;
        self.data[pos] |= 0x80u8 >> (self.bits & 7);
        pos += 1;
        // Pad with zero bits to complete (N * 512 - 256) bits.
        if pos > 32 {
            if pos < 64 {
                self.data[pos..].fill(0);
            }
            self.transform();
            pos = 0;
        }
        if pos < 32 {
            self.data[pos..32].fill(0);
        }
        let length = self.bitlength;
        self.data[32..].copy_from_slice(&length);
        self.transform();
        self.state.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u64s(&mut self.state)?;
        v.bytes(&mut self.bitlength)?;
        v.u32(&mut self.pos)?;
        v.u32(&mut self.bits)?;
        v.bytes(&mut self.data)
    }

    fn validate(&self) -> bool {
        let (pos, bits) = (self.pos as i32, self.bits as i32);
        (0..64).contains(&pos) && (pos * 8..pos * 8 + 8).contains(&bits)
    }
}
