//! Snefru-2.5a with 8 passes and 256-bit output (`snefru`, `snefru256`).

use crate::hasher::{Engine, load_be32};
use crate::state::{StateResult, Visitor};
use crate::tables::snefru::TABLES;

const SHIFTS: [u32; 4] = [16, 8, 16, 24];
const MAX32: u32 = 0xffff_ffff;

fn sbox(index: usize, byte: u32) -> u32 {
    TABLES[index * 256 + (byte & 0xff) as usize]
}

fn snefru(input: &mut [u32; 16]) {
    let mut b = *input;
    for index in 0..8 {
        let t0 = 2 * index;
        let t1 = 2 * index + 1;
        for shift in SHIFTS {
            for k in 0..16 {
                let l = (k + 15) % 16;
                let n = (k + 1) % 16;
                let table = if (k / 2) % 2 == 0 { t0 } else { t1 };
                let sbe = sbox(table, b[k]);
                b[l] ^= sbe;
                b[n] ^= sbe;
            }
            for w in &mut b {
                *w = w.rotate_right(shift);
            }
        }
    }
    for i in 0..8 {
        input[i] ^= b[15 - i];
    }
}

/// Snefru engine; the layout mirrors `PHP_SNEFRU_CTX`.
#[derive(Clone)]
pub(crate) struct Snefru {
    state: [u32; 16],
    count: [u32; 2],
    length: u8,
    buffer: [u8; 32],
}

impl Snefru {
    pub(crate) fn new() -> Self {
        Self {
            state: [0; 16],
            count: [0; 2],
            length: 0,
            buffer: [0; 32],
        }
    }

    fn transform(&mut self, input: &[u8; 32]) {
        self.state[8..].copy_from_slice(&load_be32::<8>(input));
        snefru(&mut self.state);
        self.state[8..].fill(0);
    }
}

impl Engine for Snefru {
    fn update(&mut self, input: &[u8]) {
        let len = input.len();
        // PHP's 64-bit counter arithmetic, quirks included.
        let bits = (len as u64).wrapping_mul(8);
        let bits32 = (len as u32).wrapping_mul(8);
        if u64::from(MAX32 - self.count[1]) < bits {
            self.count[0] = self.count[0].wrapping_add(1);
            self.count[1] = MAX32 - self.count[1];
            self.count[1] = bits32.wrapping_sub(self.count[1]);
        } else {
            self.count[1] = self.count[1].wrapping_add(bits32);
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
        self.state[14] = self.count[0];
        self.state[15] = self.count[1];
        snefru(&mut self.state);
        self.state[..8]
            .iter()
            .flat_map(|w| w.to_be_bytes())
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
