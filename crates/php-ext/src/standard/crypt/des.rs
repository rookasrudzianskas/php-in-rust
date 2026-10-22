//! Traditional and extended DES `crypt()` (ext/standard/crypt_freesec.c).
//!
//! This is the FreeSec / Solar Designer implementation: table driven DES with
//! the salt folded into the E-box expansion.

use std::sync::OnceLock;

const IP: [u8; 64] = [
    58, 50, 42, 34, 26, 18, 10, 2, 60, 52, 44, 36, 28, 20, 12, 4, 62, 54, 46, 38, 30, 22, 14, 6,
    64, 56, 48, 40, 32, 24, 16, 8, 57, 49, 41, 33, 25, 17, 9, 1, 59, 51, 43, 35, 27, 19, 11, 3, 61,
    53, 45, 37, 29, 21, 13, 5, 63, 55, 47, 39, 31, 23, 15, 7,
];

const KEY_PERM: [u8; 56] = [
    57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59, 51, 43, 35, 27, 19, 11, 3, 60,
    52, 44, 36, 63, 55, 47, 39, 31, 23, 15, 7, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29,
    21, 13, 5, 28, 20, 12, 4,
];

const KEY_SHIFTS: [u8; 16] = [1, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 2, 2, 2, 1];

const COMP_PERM: [u8; 48] = [
    14, 17, 11, 24, 1, 5, 3, 28, 15, 6, 21, 10, 23, 19, 12, 4, 26, 8, 16, 7, 27, 20, 13, 2, 41, 52,
    31, 37, 47, 55, 30, 40, 51, 45, 33, 48, 44, 49, 39, 56, 34, 53, 46, 42, 50, 36, 29, 32,
];

const SBOX: [[u8; 64]; 8] = [
    [
        14, 4, 13, 1, 2, 15, 11, 8, 3, 10, 6, 12, 5, 9, 0, 7, 0, 15, 7, 4, 14, 2, 13, 1, 10, 6, 12,
        11, 9, 5, 3, 8, 4, 1, 14, 8, 13, 6, 2, 11, 15, 12, 9, 7, 3, 10, 5, 0, 15, 12, 8, 2, 4, 9,
        1, 7, 5, 11, 3, 14, 10, 0, 6, 13,
    ],
    [
        15, 1, 8, 14, 6, 11, 3, 4, 9, 7, 2, 13, 12, 0, 5, 10, 3, 13, 4, 7, 15, 2, 8, 14, 12, 0, 1,
        10, 6, 9, 11, 5, 0, 14, 7, 11, 10, 4, 13, 1, 5, 8, 12, 6, 9, 3, 2, 15, 13, 8, 10, 1, 3, 15,
        4, 2, 11, 6, 7, 12, 0, 5, 14, 9,
    ],
    [
        10, 0, 9, 14, 6, 3, 15, 5, 1, 13, 12, 7, 11, 4, 2, 8, 13, 7, 0, 9, 3, 4, 6, 10, 2, 8, 5,
        14, 12, 11, 15, 1, 13, 6, 4, 9, 8, 15, 3, 0, 11, 1, 2, 12, 5, 10, 14, 7, 1, 10, 13, 0, 6,
        9, 8, 7, 4, 15, 14, 3, 11, 5, 2, 12,
    ],
    [
        7, 13, 14, 3, 0, 6, 9, 10, 1, 2, 8, 5, 11, 12, 4, 15, 13, 8, 11, 5, 6, 15, 0, 3, 4, 7, 2,
        12, 1, 10, 14, 9, 10, 6, 9, 0, 12, 11, 7, 13, 15, 1, 3, 14, 5, 2, 8, 4, 3, 15, 0, 6, 10, 1,
        13, 8, 9, 4, 5, 11, 12, 7, 2, 14,
    ],
    [
        2, 12, 4, 1, 7, 10, 11, 6, 8, 5, 3, 15, 13, 0, 14, 9, 14, 11, 2, 12, 4, 7, 13, 1, 5, 0, 15,
        10, 3, 9, 8, 6, 4, 2, 1, 11, 10, 13, 7, 8, 15, 9, 12, 5, 6, 3, 0, 14, 11, 8, 12, 7, 1, 14,
        2, 13, 6, 15, 0, 9, 10, 4, 5, 3,
    ],
    [
        12, 1, 10, 15, 9, 2, 6, 8, 0, 13, 3, 4, 14, 7, 5, 11, 10, 15, 4, 2, 7, 12, 9, 5, 6, 1, 13,
        14, 0, 11, 3, 8, 9, 14, 15, 5, 2, 8, 12, 3, 7, 0, 4, 10, 1, 13, 11, 6, 4, 3, 2, 12, 9, 5,
        15, 10, 11, 14, 1, 7, 6, 0, 8, 13,
    ],
    [
        4, 11, 2, 14, 15, 0, 8, 13, 3, 12, 9, 7, 5, 10, 6, 1, 13, 0, 11, 7, 4, 9, 1, 10, 14, 3, 5,
        12, 2, 15, 8, 6, 1, 4, 11, 13, 12, 3, 7, 14, 10, 15, 6, 8, 0, 5, 9, 2, 6, 11, 13, 8, 1, 4,
        10, 7, 9, 5, 0, 15, 14, 2, 3, 12,
    ],
    [
        13, 2, 8, 4, 6, 15, 11, 1, 10, 9, 3, 14, 5, 0, 12, 7, 1, 15, 13, 8, 10, 3, 7, 4, 12, 5, 6,
        11, 0, 14, 9, 2, 7, 11, 4, 1, 9, 12, 14, 2, 0, 6, 10, 13, 15, 3, 5, 8, 2, 1, 14, 7, 4, 10,
        8, 13, 15, 12, 9, 0, 3, 5, 6, 11,
    ],
];

const PBOX: [u8; 32] = [
    16, 7, 20, 21, 29, 12, 28, 17, 1, 15, 23, 26, 5, 18, 31, 10, 2, 8, 24, 14, 32, 27, 3, 9, 19,
    13, 30, 6, 22, 11, 4, 25,
];

const BITS32: [u32; 32] = [
    0x80000000, 0x40000000, 0x20000000, 0x10000000, 0x08000000, 0x04000000, 0x02000000, 0x01000000,
    0x00800000, 0x00400000, 0x00200000, 0x00100000, 0x00080000, 0x00040000, 0x00020000, 0x00010000,
    0x00008000, 0x00004000, 0x00002000, 0x00001000, 0x00000800, 0x00000400, 0x00000200, 0x00000100,
    0x00000080, 0x00000040, 0x00000020, 0x00000010, 0x00000008, 0x00000004, 0x00000002, 0x00000001,
];

const BITS8: [u8; 8] = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];

const ASCII64: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The lookup tables `_crypt_extended_init()` builds once.
struct Tables {
    m_sbox: [[u8; 4096]; 4],
    psbox: [[u32; 256]; 4],
    ip_maskl: [[u32; 256]; 8],
    ip_maskr: [[u32; 256]; 8],
    fp_maskl: [[u32; 256]; 8],
    fp_maskr: [[u32; 256]; 8],
    key_perm_maskl: [[u32; 128]; 8],
    key_perm_maskr: [[u32; 128]; 8],
    comp_maskl: [[u32; 128]; 8],
    comp_maskr: [[u32; 128]; 8],
}

impl Tables {
    /// `_crypt_extended_init()`.
    fn build() -> Box<Tables> {
        let mut t = Box::new(Tables {
            m_sbox: [[0; 4096]; 4],
            psbox: [[0; 256]; 4],
            ip_maskl: [[0; 256]; 8],
            ip_maskr: [[0; 256]; 8],
            fp_maskl: [[0; 256]; 8],
            fp_maskr: [[0; 256]; 8],
            key_perm_maskl: [[0; 128]; 8],
            key_perm_maskr: [[0; 128]; 8],
            comp_maskl: [[0; 128]; 8],
            comp_maskr: [[0; 128]; 8],
        });
        let bits28 = &BITS32[4..];
        let bits24 = &BITS32[8..];

        // Invert the S-boxes, reordering the input bits.
        let mut u_sbox = [[0u8; 64]; 8];
        for (u_row, row) in u_sbox.iter_mut().zip(&SBOX) {
            for (j, u) in u_row.iter_mut().enumerate() {
                let b = (j & 0x20) | ((j & 1) << 4) | ((j >> 1) & 0xf);
                *u = row[b];
            }
        }

        // Convert the inverted S-boxes into 4 arrays of 8 bits; each handles
        // 12 bits of the S-box input.
        for b in 0..4 {
            for i in 0..64 {
                for j in 0..64 {
                    t.m_sbox[b][(i << 6) | j] = (u_sbox[b << 1][i] << 4) | u_sbox[(b << 1) + 1][j];
                }
            }
        }

        // Initial & final permutations, inverted key permutation.
        let mut init_perm = [0u8; 64];
        let mut final_perm = [0u8; 64];
        let mut inv_key_perm = [255u8; 64];
        let mut inv_comp_perm = [255u8; 56];
        for i in 0..64 {
            final_perm[i] = IP[i] - 1;
            init_perm[final_perm[i] as usize] = i as u8;
        }
        for i in 0..56 {
            inv_key_perm[KEY_PERM[i] as usize - 1] = i as u8;
        }
        for i in 0..48 {
            inv_comp_perm[COMP_PERM[i] as usize - 1] = i as u8;
        }

        // OR-mask arrays for the initial and final permutations, and for the
        // key initial and compression permutations.
        for k in 0..8 {
            for i in 0..256usize {
                let (mut il, mut ir, mut fl, mut fr) = (0u32, 0u32, 0u32, 0u32);
                for (j, bit) in BITS8.iter().enumerate() {
                    let inbit = 8 * k + j;
                    if i & *bit as usize != 0 {
                        let obit = init_perm[inbit] as usize;
                        if obit < 32 {
                            il |= BITS32[obit];
                        } else {
                            ir |= BITS32[obit - 32];
                        }
                        let obit = final_perm[inbit] as usize;
                        if obit < 32 {
                            fl |= BITS32[obit];
                        } else {
                            fr |= BITS32[obit - 32];
                        }
                    }
                }
                t.ip_maskl[k][i] = il;
                t.ip_maskr[k][i] = ir;
                t.fp_maskl[k][i] = fl;
                t.fp_maskr[k][i] = fr;
            }
            for i in 0..128usize {
                let (mut il, mut ir) = (0u32, 0u32);
                for j in 0..7 {
                    let inbit = 8 * k + j;
                    if i & BITS8[j + 1] as usize != 0 {
                        let obit = inv_key_perm[inbit];
                        if obit == 255 {
                            continue;
                        }
                        if obit < 28 {
                            il |= bits28[obit as usize];
                        } else {
                            ir |= bits28[obit as usize - 28];
                        }
                    }
                }
                t.key_perm_maskl[k][i] = il;
                t.key_perm_maskr[k][i] = ir;

                let (mut il, mut ir) = (0u32, 0u32);
                for j in 0..7 {
                    let inbit = 7 * k + j;
                    if i & BITS8[j + 1] as usize != 0 {
                        let obit = inv_comp_perm[inbit];
                        if obit == 255 {
                            continue;
                        }
                        if obit < 24 {
                            il |= bits24[obit as usize];
                        } else {
                            ir |= bits24[obit as usize - 24];
                        }
                    }
                }
                t.comp_maskl[k][i] = il;
                t.comp_maskr[k][i] = ir;
            }
        }

        // Invert the P-box permutation and convert it into OR-masks for the
        // output of the S-box arrays.
        let mut un_pbox = [0u8; 32];
        for i in 0..32 {
            un_pbox[PBOX[i] as usize - 1] = i as u8;
        }
        for b in 0..4 {
            for i in 0..256usize {
                let mut p = 0u32;
                for j in 0..8 {
                    if i & BITS8[j] as usize != 0 {
                        p |= BITS32[un_pbox[8 * b + j] as usize];
                    }
                }
                t.psbox[b][i] = p;
            }
        }
        t
    }
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Box<Tables>> = OnceLock::new();
    TABLES.get_or_init(Tables::build)
}

/// `ascii_to_bin()`; `ch` is read as a signed `char`.
fn ascii_to_bin(ch: u8) -> i32 {
    let sch = ch as i8 as i32;
    let mut retval = sch - b'.' as i32;
    if sch >= b'A' as i32 {
        retval = sch - (b'A' as i32 - 12);
        if sch >= b'a' as i32 {
            retval = sch - (b'a' as i32 - 38);
        }
    }
    retval & 0x3f
}

/// `ascii_is_unsafe()`: characters that would break the passwd file format.
fn ascii_is_unsafe(ch: u8) -> bool {
    ch == 0 || ch == b'\n' || ch == b':'
}

/// Key schedule and salt state (`struct php_crypt_extended_data`).
struct State {
    saltbits: u32,
    en_keysl: [u32; 16],
    en_keysr: [u32; 16],
}

impl State {
    fn new() -> State {
        State {
            saltbits: 0,
            en_keysl: [0; 16],
            en_keysr: [0; 16],
        }
    }

    /// `setup_salt()`.
    fn setup_salt(&mut self, salt: u32) {
        let mut saltbits = 0u32;
        let mut saltbit = 1u32;
        let mut obit = 0x800000u32;
        for _ in 0..24 {
            if salt & saltbit != 0 {
                saltbits |= obit;
            }
            saltbit <<= 1;
            obit >>= 1;
        }
        self.saltbits = saltbits;
    }

    /// `des_setkey()`.
    fn set_key(&mut self, t: &Tables, key: &[u8; 8]) {
        let rawkey0 = u32::from_be_bytes([key[0], key[1], key[2], key[3]]);
        let rawkey1 = u32::from_be_bytes([key[4], key[5], key[6], key[7]]);

        // Key permutation, split into two 28-bit subkeys.
        let k0 = t.key_perm_maskl[0][(rawkey0 >> 25) as usize]
            | t.key_perm_maskl[1][((rawkey0 >> 17) & 0x7f) as usize]
            | t.key_perm_maskl[2][((rawkey0 >> 9) & 0x7f) as usize]
            | t.key_perm_maskl[3][((rawkey0 >> 1) & 0x7f) as usize]
            | t.key_perm_maskl[4][(rawkey1 >> 25) as usize]
            | t.key_perm_maskl[5][((rawkey1 >> 17) & 0x7f) as usize]
            | t.key_perm_maskl[6][((rawkey1 >> 9) & 0x7f) as usize]
            | t.key_perm_maskl[7][((rawkey1 >> 1) & 0x7f) as usize];
        let k1 = t.key_perm_maskr[0][(rawkey0 >> 25) as usize]
            | t.key_perm_maskr[1][((rawkey0 >> 17) & 0x7f) as usize]
            | t.key_perm_maskr[2][((rawkey0 >> 9) & 0x7f) as usize]
            | t.key_perm_maskr[3][((rawkey0 >> 1) & 0x7f) as usize]
            | t.key_perm_maskr[4][(rawkey1 >> 25) as usize]
            | t.key_perm_maskr[5][((rawkey1 >> 17) & 0x7f) as usize]
            | t.key_perm_maskr[6][((rawkey1 >> 9) & 0x7f) as usize]
            | t.key_perm_maskr[7][((rawkey1 >> 1) & 0x7f) as usize];

        // Rotate subkeys and do the compression permutation.
        let mut shifts = 0u32;
        for (round, shift) in KEY_SHIFTS.iter().enumerate() {
            shifts += *shift as u32;
            let t0 = (k0 << shifts) | (k0 >> (28 - shifts));
            let t1 = (k1 << shifts) | (k1 >> (28 - shifts));

            self.en_keysl[round] = t.comp_maskl[0][((t0 >> 21) & 0x7f) as usize]
                | t.comp_maskl[1][((t0 >> 14) & 0x7f) as usize]
                | t.comp_maskl[2][((t0 >> 7) & 0x7f) as usize]
                | t.comp_maskl[3][(t0 & 0x7f) as usize]
                | t.comp_maskl[4][((t1 >> 21) & 0x7f) as usize]
                | t.comp_maskl[5][((t1 >> 14) & 0x7f) as usize]
                | t.comp_maskl[6][((t1 >> 7) & 0x7f) as usize]
                | t.comp_maskl[7][(t1 & 0x7f) as usize];

            self.en_keysr[round] = t.comp_maskr[0][((t0 >> 21) & 0x7f) as usize]
                | t.comp_maskr[1][((t0 >> 14) & 0x7f) as usize]
                | t.comp_maskr[2][((t0 >> 7) & 0x7f) as usize]
                | t.comp_maskr[3][(t0 & 0x7f) as usize]
                | t.comp_maskr[4][((t1 >> 21) & 0x7f) as usize]
                | t.comp_maskr[5][((t1 >> 14) & 0x7f) as usize]
                | t.comp_maskr[6][((t1 >> 7) & 0x7f) as usize]
                | t.comp_maskr[7][(t1 & 0x7f) as usize];
        }
    }

    /// `do_des()` for encryption (`count > 0`); `None` is php-src's non-zero
    /// return for `count == 0`.
    fn do_des(&self, t: &Tables, l_in: u32, r_in: u32, count: u32) -> Option<(u32, u32)> {
        if count == 0 {
            return None;
        }

        // Initial permutation (IP).
        let mut l = t.ip_maskl[0][(l_in >> 24) as usize]
            | t.ip_maskl[1][((l_in >> 16) & 0xff) as usize]
            | t.ip_maskl[2][((l_in >> 8) & 0xff) as usize]
            | t.ip_maskl[3][(l_in & 0xff) as usize]
            | t.ip_maskl[4][(r_in >> 24) as usize]
            | t.ip_maskl[5][((r_in >> 16) & 0xff) as usize]
            | t.ip_maskl[6][((r_in >> 8) & 0xff) as usize]
            | t.ip_maskl[7][(r_in & 0xff) as usize];
        let mut r = t.ip_maskr[0][(l_in >> 24) as usize]
            | t.ip_maskr[1][((l_in >> 16) & 0xff) as usize]
            | t.ip_maskr[2][((l_in >> 8) & 0xff) as usize]
            | t.ip_maskr[3][(l_in & 0xff) as usize]
            | t.ip_maskr[4][(r_in >> 24) as usize]
            | t.ip_maskr[5][((r_in >> 16) & 0xff) as usize]
            | t.ip_maskr[6][((r_in >> 8) & 0xff) as usize]
            | t.ip_maskr[7][(r_in & 0xff) as usize];

        let saltbits = self.saltbits;
        let mut f = 0u32;
        for _ in 0..count {
            for round in 0..16 {
                // Expand R to 48 bits (simulate the E-box).
                let mut r48l = ((r & 0x00000001) << 23)
                    | ((r & 0xf8000000) >> 9)
                    | ((r & 0x1f800000) >> 11)
                    | ((r & 0x01f80000) >> 13)
                    | ((r & 0x001f8000) >> 15);
                let mut r48r = ((r & 0x0001f800) << 7)
                    | ((r & 0x00001f80) << 5)
                    | ((r & 0x000001f8) << 3)
                    | ((r & 0x0000001f) << 1)
                    | ((r & 0x80000000) >> 31);
                // Salting, then XOR with the permuted key.
                f = (r48l ^ r48r) & saltbits;
                r48l ^= f ^ self.en_keysl[round];
                r48r ^= f ^ self.en_keysr[round];
                // S-box lookups (shrinking back to 32 bits) and the P-box.
                f = t.psbox[0][t.m_sbox[0][(r48l >> 12) as usize] as usize]
                    | t.psbox[1][t.m_sbox[1][(r48l & 0xfff) as usize] as usize]
                    | t.psbox[2][t.m_sbox[2][(r48r >> 12) as usize] as usize]
                    | t.psbox[3][t.m_sbox[3][(r48r & 0xfff) as usize] as usize];
                f ^= l;
                l = r;
                r = f;
            }
            r = l;
            l = f;
        }

        // Final permutation (inverse of IP).
        let l_out = t.fp_maskl[0][(l >> 24) as usize]
            | t.fp_maskl[1][((l >> 16) & 0xff) as usize]
            | t.fp_maskl[2][((l >> 8) & 0xff) as usize]
            | t.fp_maskl[3][(l & 0xff) as usize]
            | t.fp_maskl[4][(r >> 24) as usize]
            | t.fp_maskl[5][((r >> 16) & 0xff) as usize]
            | t.fp_maskl[6][((r >> 8) & 0xff) as usize]
            | t.fp_maskl[7][(r & 0xff) as usize];
        let r_out = t.fp_maskr[0][(l >> 24) as usize]
            | t.fp_maskr[1][((l >> 16) & 0xff) as usize]
            | t.fp_maskr[2][((l >> 8) & 0xff) as usize]
            | t.fp_maskr[3][(l & 0xff) as usize]
            | t.fp_maskr[4][(r >> 24) as usize]
            | t.fp_maskr[5][((r >> 16) & 0xff) as usize]
            | t.fp_maskr[6][((r >> 8) & 0xff) as usize]
            | t.fp_maskr[7][(r & 0xff) as usize];
        Some((l_out, r_out))
    }

    /// `des_cipher(in, out, 0, 1)`: encrypt one block with salt 0.
    fn cipher_block(&mut self, t: &Tables, block: &[u8; 8]) -> Option<[u8; 8]> {
        self.setup_salt(0);
        let rawl = u32::from_be_bytes([block[0], block[1], block[2], block[3]]);
        let rawr = u32::from_be_bytes([block[4], block[5], block[6], block[7]]);
        let (l_out, r_out) = self.do_des(t, rawl, rawr, 1)?;
        let mut out = [0u8; 8];
        out[..4].copy_from_slice(&l_out.to_be_bytes());
        out[4..].copy_from_slice(&r_out.to_be_bytes());
        Some(out)
    }
}

/// `_crypt_extended_r()`.
///
/// `key` and `setting` are the contents of the C strings php-src passes (no
/// terminating NUL); reads past `setting`'s end see NUL like the C code does.
/// `None` is php-src's NULL return.
pub fn crypt_extended(key: &[u8], setting: &[u8]) -> Option<Vec<u8>> {
    let t = tables();
    let mut st = State::new();
    let set = |i: usize| setting.get(i).copied().unwrap_or(0);

    // Copy the key, shifting each character up by one bit and padding with
    // zeros.
    let mut ki = 0usize;
    let mut keybuf = [0u8; 8];
    for q in keybuf.iter_mut() {
        let c = key.get(ki).copied().unwrap_or(0);
        *q = c << 1;
        if c != 0 {
            ki += 1;
        }
    }
    st.set_key(t, &keybuf);

    let count: u32;
    let salt: u32;
    let mut out: Vec<u8> = Vec::with_capacity(20);
    if set(0) == b'_' {
        // "new"-style: underscore, 4 chars of count, 4 chars of salt; the key
        // has unlimited characters.
        let mut cnt = 0u32;
        for i in 1..5 {
            let value = ascii_to_bin(set(i));
            if ASCII64[value as usize] != set(i) {
                return None;
            }
            cnt |= (value as u32) << ((i - 1) * 6);
        }
        if cnt == 0 {
            return None;
        }
        let mut slt = 0u32;
        for i in 5..9 {
            let value = ascii_to_bin(set(i));
            if ASCII64[value as usize] != set(i) {
                return None;
            }
            slt |= (value as u32) << ((i - 5) * 6);
        }
        count = cnt;
        salt = slt;

        while ki < key.len() && key[ki] != 0 {
            // Encrypt the key with itself.
            keybuf = st.cipher_block(t, &keybuf)?;
            // And XOR with the next 8 characters of the key.
            let mut q = 0;
            while q < 8 && ki < key.len() && key[ki] != 0 {
                keybuf[q] ^= key[ki] << 1;
                q += 1;
                ki += 1;
            }
            st.set_key(t, &keybuf);
        }
        out.extend_from_slice(&[
            set(0),
            set(1),
            set(2),
            set(3),
            set(4),
            set(5),
            set(6),
            set(7),
            set(8),
        ]);
    } else {
        // "old"-style: 2 chars of salt, key up to 8 characters.
        count = 25;
        if ascii_is_unsafe(set(0)) || ascii_is_unsafe(set(1)) {
            return None;
        }
        salt = ((ascii_to_bin(set(1)) << 6) | ascii_to_bin(set(0))) as u32;
        out.push(set(0));
        out.push(set(1));
    }
    st.setup_salt(salt);
    let (r0, r1) = st.do_des(t, 0, 0, count)?;

    // Encode the result.
    let mut l = r0 >> 8;
    out.push(ASCII64[((l >> 18) & 0x3f) as usize]);
    out.push(ASCII64[((l >> 12) & 0x3f) as usize]);
    out.push(ASCII64[((l >> 6) & 0x3f) as usize]);
    out.push(ASCII64[(l & 0x3f) as usize]);

    l = (r0 << 16) | ((r1 >> 16) & 0xffff);
    out.push(ASCII64[((l >> 18) & 0x3f) as usize]);
    out.push(ASCII64[((l >> 12) & 0x3f) as usize]);
    out.push(ASCII64[((l >> 6) & 0x3f) as usize]);
    out.push(ASCII64[(l & 0x3f) as usize]);

    l = r1 << 2;
    out.push(ASCII64[((l >> 12) & 0x3f) as usize]);
    out.push(ASCII64[((l >> 6) & 0x3f) as usize]);
    out.push(ASCII64[(l & 0x3f) as usize]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn des(key: &[u8], setting: &[u8]) -> Option<String> {
        crypt_extended(key, setting).map(|v| String::from_utf8_lossy(&v).into_owned())
    }

    // The self-test table from crypt_freesec.c (`#ifdef TEST`).
    #[test]
    fn freesec_self_test_vectors() {
        let tests: &[(&[u8], &[u8])] = &[
            (b"_J9..CCCCXBrJUJV154M", b"U*U*U*U*"),
            (b"_J9..CCCCXUhOBTXzaiE", b"U*U***U"),
            (b"_J9..CCCC4gQ.mB/PffM", b"U*U***U*"),
            (b"_J9..XXXXvlzQGqpPPdk", b"*U*U*U*U"),
            (b"_J9..XXXXsqM/YSSP..Y", b"*U*U*U*U*"),
            (b"_J9..XXXXVL7qJCnku0I", b"*U*U*U*U*U*U*U*U"),
            (b"_J9..XXXXAj8cFbP5scI", b"*U*U*U*U*U*U*U*U*"),
            (b"_J9..SDizh.vll5VED9g", b"ab1234567"),
            (b"_J9..SDizRjWQ/zePPHc", b"cr1234567"),
            (b"_J9..SDizxmRI1GjnQuE", b"zxyDPWgydbQjgq"),
            (b"_K9..SaltNrQgIYUAeoY", b"726 even"),
            (b"_J9..SDSD5YGyRCr4W4c", b""),
            (b"CCNf8Sbh3HDfQ", b"U*U*U*U*"),
            (b"CCX.K.MFy4Ois", b"U*U***U"),
            (b"CC4rMpbg9AMZ.", b"U*U***U*"),
            (b"XXxzOu6maQKqQ", b"*U*U*U*U"),
            (b"SDbsugeBiC58A", b""),
            (b"./xZjzHv5vzVE", b"password"),
            (b"0A2hXM1rXbYgo", b"password"),
            (b"A9RXdR23Y.cY6", b"password"),
            (b"ZziFATVXHo2.6", b"password"),
            (b"zZDDIZ0NOlPzw", b"password"),
            // "reasonable" invalid salts, UFC-crypt behaviour expected
            (b"\x01\x02wyd0KZo65Jo", b"password"),
            (b"a_C10Dk/ExaG.", b"password"),
            (b"~\xff.5OTsRVjwLo", b"password"),
        ];
        for (hash, pw) in tests {
            let got = crypt_extended(pw, hash).unwrap_or_else(|| panic!("NULL for {:?}", hash));
            assert_eq!(got, *hash);
        }
    }

    #[test]
    fn freesec_erroneous_settings() {
        for setting in [
            &b""[..],
            b" ",
            b"a:",
            b"\na",
            b"_/......",
            b"_........",
            b"_/!......",
            b"_/......!",
        ] {
            assert_eq!(des(b"", setting), None, "{setting:?}");
        }
    }

    #[test]
    fn php_documentation_vectors() {
        assert_eq!(des(b"rasmuslerdorf", b"rl").unwrap(), "rl.3StKT.4T8M");
        assert_eq!(
            des(b"rasmuslerdorf", b"_J9..rasm").unwrap(),
            "_J9..rasmBYk8r9AiWNc"
        );
    }

    #[test]
    fn high_bit_characters() {
        // ext/standard/tests/strings/crypt_chars.phpt
        assert_eq!(des("À1234abcd".as_bytes(), b"99").unwrap(), "99PxawtsTfX56");
        assert_eq!(des("À9234abcd".as_bytes(), b"99").unwrap(), "99jcVcGxUZOWk");
        assert_eq!(
            des("À1234abcd".as_bytes(), b"_01234567").unwrap(),
            "_01234567IBjxKliXXRQ"
        );
        assert_eq!(
            des("À9234abcd".as_bytes(), b"_01234567").unwrap(),
            "_012345678OSGpGQRVHA"
        );
    }
}
