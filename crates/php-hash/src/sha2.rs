//! SHA-224, SHA-256, SHA-384, SHA-512, SHA-512/224 and SHA-512/256.

use crate::hasher::{
    Engine, Md32, PADDING, Transform32, encode32, load_be32, load_be64, rsa_update,
};
use crate::state::{StateResult, Visitor};

/// SHA-224 / SHA-256 (`sha224`, `sha256`).
#[derive(Clone)]
pub(crate) struct Sha256 {
    core: Md32<8>,
    digest_len: usize,
}

impl Sha256 {
    pub(crate) fn new_256() -> Self {
        Self {
            core: Md32::new([
                0x6a09_e667,
                0xbb67_ae85,
                0x3c6e_f372,
                0xa54f_f53a,
                0x510e_527f,
                0x9b05_688c,
                0x1f83_d9ab,
                0x5be0_cd19,
            ]),
            digest_len: 32,
        }
    }

    pub(crate) fn new_224() -> Self {
        Self {
            core: Md32::new([
                0xc105_9ed8,
                0x367c_d507,
                0x3070_dd17,
                0xf70e_5939,
                0xffc0_0b31,
                0x6858_1511,
                0x64f9_8fa7,
                0xbefa_4fa4,
            ]),
            digest_len: 28,
        }
    }
}

fn sha256_transform(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    w[..16].copy_from_slice(&load_be32::<16>(block));
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K256[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

impl Engine for Sha256 {
    fn update(&mut self, data: &[u8]) {
        self.core.update(data, sha256_transform as Transform32<8>);
    }

    fn finish(mut self) -> Vec<u8> {
        self.core.pad(sha256_transform, true);
        encode32(&self.core.state, true, self.digest_len)
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        self.core.visit(v)
    }
}

/// SHA-384 / SHA-512 / SHA-512/224 / SHA-512/256.
#[derive(Clone)]
pub(crate) struct Sha512 {
    state: [u64; 8],
    /// Number of bits processed, modulo 2^128 (PHP's `count[2]`).
    count: u128,
    buffer: [u8; 128],
    digest_len: usize,
}

impl Sha512 {
    fn with(state: [u64; 8], digest_len: usize) -> Self {
        Self {
            state,
            count: 0,
            buffer: [0; 128],
            digest_len,
        }
    }

    pub(crate) fn new_512() -> Self {
        Self::with(
            [
                0x6a09_e667_f3bc_c908,
                0xbb67_ae85_84ca_a73b,
                0x3c6e_f372_fe94_f82b,
                0xa54f_f53a_5f1d_36f1,
                0x510e_527f_ade6_82d1,
                0x9b05_688c_2b3e_6c1f,
                0x1f83_d9ab_fb41_bd6b,
                0x5be0_cd19_137e_2179,
            ],
            64,
        )
    }

    pub(crate) fn new_384() -> Self {
        Self::with(
            [
                0xcbbb_9d5d_c105_9ed8,
                0x629a_292a_367c_d507,
                0x9159_015a_3070_dd17,
                0x152f_ecd8_f70e_5939,
                0x6733_2667_ffc0_0b31,
                0x8eb4_4a87_6858_1511,
                0xdb0c_2e0d_64f9_8fa7,
                0x47b5_481d_befa_4fa4,
            ],
            48,
        )
    }

    pub(crate) fn new_512_256() -> Self {
        Self::with(
            [
                0x2231_2194_FC2B_F72C,
                0x9F55_5FA3_C84C_64C2,
                0x2393_B86B_6F53_B151,
                0x9638_7719_5940_EABD,
                0x9628_3EE2_A88E_FFE3,
                0xBE5E_1E25_5386_3992,
                0x2B01_99FC_2C85_B8AA,
                0x0EB7_2DDC_81C5_2CA2,
            ],
            32,
        )
    }

    pub(crate) fn new_512_224() -> Self {
        Self::with(
            [
                0x8C3D_37C8_1954_4DA2,
                0x73E1_9966_89DC_D4D6,
                0x1DFA_B7AE_32FF_9C82,
                0x679D_D514_582F_9FCF,
                0x0F6D_2B69_7BD4_4DA8,
                0x77E3_6F73_04C4_8942,
                0x3F9D_85A8_6A1D_36C8,
                0x1112_E6AD_91D6_92A1,
            ],
            28,
        )
    }
}

fn sha512_transform(state: &mut [u64; 8], block: &[u8; 128]) {
    let mut w = [0u64; 80];
    w[..16].copy_from_slice(&load_be64::<16>(block));
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K512[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

impl Engine for Sha512 {
    fn update(&mut self, data: &[u8]) {
        let index = ((self.count >> 3) & 0x7f) as usize;
        self.count = self.count.wrapping_add((data.len() as u128) << 3);
        let state = &mut self.state;
        rsa_update(&mut self.buffer, index, data, |b| {
            sha512_transform(state, b);
        });
    }

    fn finish(mut self) -> Vec<u8> {
        let bits = self.count.to_be_bytes();
        let index = ((self.count >> 3) & 0x7f) as usize;
        let pad_len = if index < 112 {
            112 - index
        } else {
            240 - index
        };
        self.update(&PADDING[..pad_len]);
        self.update(&bits);
        let mut out: Vec<u8> = self.state.iter().flat_map(|w| w.to_be_bytes()).collect();
        out.truncate(self.digest_len);
        out
    }

    fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u64s(&mut self.state)?;
        v.count64(&mut self.count)?;
        v.bytes(&mut self.buffer)
    }
}

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const K512: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];
