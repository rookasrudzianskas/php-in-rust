//! SHA-1.

use crate::hasher::{Engine, Md32, Transform32, encode32, load_be32};
use crate::state::{StateResult, Visitor};

/// SHA-1 (`sha1`); also backs the `sha1()` helper.
#[derive(Clone)]
pub(crate) struct Sha1(Md32<5>);

impl Sha1 {
    pub(crate) fn new() -> Self {
        Self(Md32::new([
            0x6745_2301,
            0xefcd_ab89,
            0x98ba_dcfe,
            0x1032_5476,
            0xc3d2_e1f0,
        ]))
    }
}

fn sha1_transform(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut w = [0u32; 80];
    w[..16].copy_from_slice(&load_be32::<16>(block));
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (i, &wi) in w.iter().enumerate() {
        let (f, k) = match i / 20 {
            0 => ((b & c) | (!b & d), 0x5a82_7999u32),
            1 => (b ^ c ^ d, 0x6ed9_eba1),
            2 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let t = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

impl Engine for Sha1 {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data, sha1_transform as Transform32<5>);
    }

    fn finish(mut self) -> Vec<u8> {
        self.0.pad(sha1_transform, true);
        encode32(&self.0.state, true, 20)
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        self.0.visit(v)
    }
}
