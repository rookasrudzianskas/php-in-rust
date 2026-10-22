//! BLAKE2b (RFC 7693), as needed by Argon2: sequential mode, no key, digest
//! sizes 1 to 64 bytes.

const IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

const BLOCK_BYTES: usize = 128;

/// Maximum digest size, `BLAKE2B_OUTBYTES`.
pub const OUT_BYTES: usize = 64;

/// An incremental BLAKE2b computation.
pub struct Blake2b {
    h: [u64; 8],
    /// Number of bytes compressed so far.
    t: u128,
    buf: [u8; BLOCK_BYTES],
    buflen: usize,
    outlen: usize,
}

impl Blake2b {
    /// `blake2b_init(outlen)`.
    pub fn new(outlen: usize) -> Blake2b {
        assert!((1..=OUT_BYTES).contains(&outlen));
        let mut h = IV;
        // Parameter block: digest length, no key, fanout 1, depth 1.
        h[0] ^= 0x0101_0000 ^ outlen as u64;
        Blake2b {
            h,
            t: 0,
            buf: [0; BLOCK_BYTES],
            buflen: 0,
            outlen,
        }
    }

    fn compress(&mut self, block: &[u8; BLOCK_BYTES], last: bool) {
        let mut m = [0u64; 16];
        for (i, w) in m.iter_mut().enumerate() {
            let mut b = [0u8; 8];
            b.copy_from_slice(&block[8 * i..8 * i + 8]);
            *w = u64::from_le_bytes(b);
        }
        let mut v = [0u64; 16];
        v[..8].copy_from_slice(&self.h);
        v[8..].copy_from_slice(&IV);
        v[12] ^= self.t as u64;
        v[13] ^= (self.t >> 64) as u64;
        if last {
            v[14] = !v[14];
        }

        #[inline(always)]
        fn g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(32);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(24);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(16);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(63);
        }

        for round in 0..12 {
            let s = &SIGMA[round % 10];
            g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
        }
        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }

    /// `blake2b_update()`.
    pub fn update(&mut self, mut data: &[u8]) {
        let left = self.buflen;
        let fill = BLOCK_BYTES - left;
        if data.len() > fill {
            // Fill the buffer and compress it; the last block is kept back so
            // that it can be flagged as final.
            self.buf[left..].copy_from_slice(&data[..fill]);
            self.t += BLOCK_BYTES as u128;
            let block = self.buf;
            self.compress(&block, false);
            self.buflen = 0;
            data = &data[fill..];
            while data.len() > BLOCK_BYTES {
                let mut block = [0u8; BLOCK_BYTES];
                block.copy_from_slice(&data[..BLOCK_BYTES]);
                self.t += BLOCK_BYTES as u128;
                self.compress(&block, false);
                data = &data[BLOCK_BYTES..];
            }
        }
        self.buf[self.buflen..self.buflen + data.len()].copy_from_slice(data);
        self.buflen += data.len();
    }

    /// `blake2b_final()`: the digest of `outlen` bytes.
    pub fn finalize(mut self) -> Vec<u8> {
        self.t += self.buflen as u128;
        for b in &mut self.buf[self.buflen..] {
            *b = 0;
        }
        let block = self.buf;
        self.compress(&block, true);
        let mut out = Vec::with_capacity(OUT_BYTES);
        for w in self.h {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.truncate(self.outlen);
        out
    }
}

/// One-shot `blake2b(out, outlen, in, inlen, NULL, 0)`.
pub fn blake2b(outlen: usize, input: &[u8]) -> Vec<u8> {
    let mut b = Blake2b::new(outlen);
    b.update(input);
    b.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn rfc7693_abc() {
        assert_eq!(
            hex(&blake2b(64, b"abc")),
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(
            hex(&blake2b(64, b"")),
            "786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce"
        );
    }

    #[test]
    fn shorter_digests_and_streaming() {
        // Digest size is part of the parameter block.
        assert_eq!(hex(&blake2b(32, b"abc")).len(), 64);
        assert_ne!(blake2b(32, b"abc"), blake2b(64, b"abc")[..32].to_vec());

        // Block-boundary handling: feeding in pieces equals one shot.
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        for len in [0, 1, 127, 128, 129, 255, 256, 257, 1000] {
            let one = blake2b(48, &data[..len]);
            for step in [1, 3, 64, 128, 200] {
                let mut b = Blake2b::new(48);
                for chunk in data[..len].chunks(step) {
                    b.update(chunk);
                }
                assert_eq!(b.finalize(), one, "len {len} step {step}");
            }
        }
    }
}
