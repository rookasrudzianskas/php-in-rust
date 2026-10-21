//! RIPEMD-128, RIPEMD-160, RIPEMD-256 and RIPEMD-320.

use crate::hasher::{Engine, Md32, Transform32, encode32, load_le32};
use crate::state::{StateResult, Visitor};

const K: [u32; 5] = [
    0x0000_0000,
    0x5A82_7999,
    0x6ED9_EBA1,
    0x8F1B_BCDC,
    0xA953_FD4E,
];
/// Right line constants of RIPEMD-128 and RIPEMD-256.
const KK: [u32; 4] = [0x50A2_8BE6, 0x5C4D_D124, 0x6D70_3EF3, 0x0000_0000];
/// Right line constants of RIPEMD-160 and RIPEMD-320.
const KK160: [u32; 5] = [
    0x50A2_8BE6,
    0x5C4D_D124,
    0x6D70_3EF3,
    0x7A6D_76E9,
    0x0000_0000,
];

const R: [usize; 80] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 7, 4, 13, 1, 10, 6, 15, 3, 12, 0, 9, 5,
    2, 14, 11, 8, 3, 10, 14, 4, 9, 15, 8, 1, 2, 7, 0, 6, 13, 11, 5, 12, 1, 9, 11, 10, 0, 8, 12, 4,
    13, 3, 7, 15, 14, 5, 6, 2, 4, 0, 5, 9, 7, 12, 2, 10, 14, 1, 3, 8, 11, 6, 15, 13,
];

const RR: [usize; 80] = [
    5, 14, 7, 0, 9, 2, 11, 4, 13, 6, 15, 8, 1, 10, 3, 12, 6, 11, 3, 7, 0, 13, 5, 10, 14, 15, 8, 12,
    4, 9, 1, 2, 15, 5, 1, 3, 7, 14, 6, 9, 11, 8, 12, 2, 10, 0, 4, 13, 8, 6, 4, 1, 3, 11, 15, 0, 5,
    12, 2, 13, 9, 7, 10, 14, 12, 15, 10, 4, 1, 5, 8, 7, 6, 2, 13, 14, 0, 3, 9, 11,
];

const S: [u32; 80] = [
    11, 14, 15, 12, 5, 8, 7, 9, 11, 13, 14, 15, 6, 7, 9, 8, 7, 6, 8, 13, 11, 9, 7, 15, 7, 12, 15,
    9, 11, 7, 13, 12, 11, 13, 6, 7, 14, 9, 13, 15, 14, 8, 13, 6, 5, 12, 7, 5, 11, 12, 14, 15, 14,
    15, 9, 8, 9, 14, 5, 6, 8, 6, 5, 12, 9, 15, 5, 11, 6, 8, 13, 12, 5, 12, 13, 14, 11, 8, 5, 6,
];

const SS: [u32; 80] = [
    8, 9, 9, 11, 13, 15, 15, 5, 7, 7, 8, 11, 14, 14, 12, 6, 9, 13, 15, 7, 12, 8, 9, 11, 7, 7, 12,
    7, 6, 15, 13, 11, 9, 7, 15, 11, 8, 6, 6, 14, 12, 13, 5, 14, 13, 13, 7, 5, 15, 5, 8, 11, 14, 14,
    6, 14, 6, 9, 12, 9, 12, 5, 15, 8, 8, 5, 12, 9, 12, 5, 14, 6, 8, 13, 6, 5, 15, 13, 11, 11,
];

/// The five boolean functions F0..F4.
fn f(n: usize, x: u32, y: u32, z: u32) -> u32 {
    match n {
        0 => x ^ y ^ z,
        1 => (x & y) | (!x & z),
        2 => (x | !y) ^ z,
        3 => (x & z) | (y & !z),
        _ => x ^ (y | !z),
    }
}

fn decode(block: &[u8; 64]) -> [u32; 16] {
    load_le32(block)
}

/// One step of the 128/256 variants (4 registers).
fn step4(r: &mut [u32; 4], func: usize, x: u32, k: u32, s: u32) {
    let [a, b, c, d] = *r;
    let tmp = a
        .wrapping_add(f(func, b, c, d))
        .wrapping_add(x)
        .wrapping_add(k)
        .rotate_left(s);
    *r = [d, tmp, b, c];
}

/// One step of the 160/320 variants (5 registers).
fn step5(r: &mut [u32; 5], func: usize, x: u32, k: u32, s: u32) {
    let [a, b, c, d, e] = *r;
    let tmp = a
        .wrapping_add(f(func, b, c, d))
        .wrapping_add(x)
        .wrapping_add(k)
        .rotate_left(s)
        .wrapping_add(e);
    *r = [e, tmp, b, c.rotate_left(10), d];
}

fn ripemd128_transform(state: &mut [u32; 4], block: &[u8; 64]) {
    let x = decode(block);
    let mut l = *state;
    let mut r = *state;
    for j in 0..64 {
        let rnd = j / 16;
        step4(&mut l, rnd, x[R[j]], K[rnd], S[j]);
        step4(&mut r, 3 - rnd, x[RR[j]], KK[rnd], SS[j]);
    }
    let [a, b, c, d] = l;
    let [aa, bb, cc, dd] = r;
    let tmp = state[1].wrapping_add(c).wrapping_add(dd);
    state[1] = state[2].wrapping_add(d).wrapping_add(aa);
    state[2] = state[3].wrapping_add(a).wrapping_add(bb);
    state[3] = state[0].wrapping_add(b).wrapping_add(cc);
    state[0] = tmp;
}

fn ripemd256_transform(state: &mut [u32; 8], block: &[u8; 64]) {
    let x = decode(block);
    let mut l = [state[0], state[1], state[2], state[3]];
    let mut r = [state[4], state[5], state[6], state[7]];
    for j in 0..64 {
        let rnd = j / 16;
        step4(&mut l, rnd, x[R[j]], K[rnd], S[j]);
        step4(&mut r, 3 - rnd, x[RR[j]], KK[rnd], SS[j]);
        if j % 16 == 15 {
            std::mem::swap(&mut l[rnd], &mut r[rnd]);
        }
    }
    for i in 0..4 {
        state[i] = state[i].wrapping_add(l[i]);
        state[i + 4] = state[i + 4].wrapping_add(r[i]);
    }
}

fn ripemd160_transform(state: &mut [u32; 5], block: &[u8; 64]) {
    let x = decode(block);
    let mut l = *state;
    let mut r = *state;
    for j in 0..80 {
        let rnd = j / 16;
        step5(&mut l, rnd, x[R[j]], K[rnd], S[j]);
        step5(&mut r, 4 - rnd, x[RR[j]], KK160[rnd], SS[j]);
    }
    let [a, b, c, d, e] = l;
    let [aa, bb, cc, dd, ee] = r;
    let tmp = state[1].wrapping_add(c).wrapping_add(dd);
    state[1] = state[2].wrapping_add(d).wrapping_add(ee);
    state[2] = state[3].wrapping_add(e).wrapping_add(aa);
    state[3] = state[4].wrapping_add(a).wrapping_add(bb);
    state[4] = state[0].wrapping_add(b).wrapping_add(cc);
    state[0] = tmp;
}

fn ripemd320_transform(state: &mut [u32; 10], block: &[u8; 64]) {
    let x = decode(block);
    let mut l = [state[0], state[1], state[2], state[3], state[4]];
    let mut r = [state[5], state[6], state[7], state[8], state[9]];
    // Register exchanged between the two lines after each round.
    const SWAP: [usize; 5] = [1, 3, 0, 2, 4];
    for j in 0..80 {
        let rnd = j / 16;
        step5(&mut l, rnd, x[R[j]], K[rnd], S[j]);
        step5(&mut r, 4 - rnd, x[RR[j]], KK160[rnd], SS[j]);
        if j % 16 == 15 {
            std::mem::swap(&mut l[SWAP[rnd]], &mut r[SWAP[rnd]]);
        }
    }
    for i in 0..5 {
        state[i] = state[i].wrapping_add(l[i]);
        state[i + 5] = state[i + 5].wrapping_add(r[i]);
    }
}

/// RIPEMD family engine, generic over the number of state words.
#[derive(Clone)]
pub(crate) struct Ripemd<const S: usize> {
    core: Md32<S>,
    transform: Transform32<S>,
}

impl<const S: usize> Engine for Ripemd<S> {
    fn update(&mut self, data: &[u8]) {
        self.core.update(data, self.transform);
    }

    fn finish(mut self) -> Vec<u8> {
        self.core.pad(self.transform, false);
        encode32(&self.core.state, false, S * 4)
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        self.core.visit(v)
    }
}

pub(crate) fn ripemd128() -> Ripemd<4> {
    Ripemd {
        core: Md32::new([0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476]),
        transform: ripemd128_transform,
    }
}

pub(crate) fn ripemd160() -> Ripemd<5> {
    Ripemd {
        core: Md32::new([
            0x6745_2301,
            0xEFCD_AB89,
            0x98BA_DCFE,
            0x1032_5476,
            0xC3D2_E1F0,
        ]),
        transform: ripemd160_transform,
    }
}

pub(crate) fn ripemd256() -> Ripemd<8> {
    Ripemd {
        core: Md32::new([
            0x6745_2301,
            0xEFCD_AB89,
            0x98BA_DCFE,
            0x1032_5476,
            0x7654_3210,
            0xFEDC_BA98,
            0x89AB_CDEF,
            0x0123_4567,
        ]),
        transform: ripemd256_transform,
    }
}

pub(crate) fn ripemd320() -> Ripemd<10> {
    Ripemd {
        core: Md32::new([
            0x6745_2301,
            0xEFCD_AB89,
            0x98BA_DCFE,
            0x1032_5476,
            0xC3D2_E1F0,
            0x7654_3210,
            0xFEDC_BA98,
            0x89AB_CDEF,
            0x0123_4567,
            0x3C2D_1E0F,
        ]),
        transform: ripemd320_transform,
    }
}
