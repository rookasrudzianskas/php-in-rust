//! A compact zlib/DEFLATE decoder (RFC 1950/1951), just enough to emulate the
//! `uncompress()` calls `php_handle_swc()` makes on compressed SWF headers.
//!
//! Only the first `keep` bytes of the output are retained: the SWF code needs
//! the header of the stream, while the *success or failure* of decompressing
//! the whole stream within a size limit decides whether getimagesize()
//! reports the image. The decoder therefore runs in constant memory (a 32 KiB
//! window), however large the declared output is.

/// Outcome of an `uncompress()` call, as far as `php_handle_swc` cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// `Z_OK`: the stream ended correctly within the limits.
    Ok,
    /// `Z_BUF_ERROR`: the output limit was reached before the stream ended.
    BufError,
    /// `Z_DATA_ERROR`: corrupt or truncated input, preset dictionary required.
    DataError,
}

pub struct Outcome {
    pub status: Status,
    /// The first `keep` bytes produced (fewer if less output was produced).
    pub head: Vec<u8>,
    /// `stream.total_out`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub total_out: u64,
}

const WINDOW: usize = 32768;
const MAXBITS: usize = 15;

/// Decompresses the zlib stream `input`, producing at most `max_out` bytes.
pub fn zlib_inflate(input: &[u8], max_out: u64, keep: usize) -> Outcome {
    let mut d = Decoder {
        input,
        pos: 0,
        bitbuf: 0,
        bitcnt: 0,
        window: vec![0u8; WINDOW],
        wpos: 0,
        total_out: 0,
        max_out,
        keep,
        head: Vec::new(),
        adler_a: 1,
        adler_b: 0,
    };
    let status = match d.run() {
        Ok(()) => Status::Ok,
        Err(Stop::Output) => Status::BufError,
        Err(Stop::Data) => Status::DataError,
    };
    Outcome {
        status,
        head: d.head,
        total_out: d.total_out,
    }
}

enum Stop {
    /// Output limit reached.
    Output,
    /// Corrupt or truncated input.
    Data,
}

struct Huffman {
    count: [u16; MAXBITS + 1],
    symbol: Vec<u16>,
}

impl Huffman {
    /// puff.c `construct`: returns the table and the `left` value
    /// (negative: over-subscribed, positive: incomplete).
    fn new(lengths: &[u8]) -> (Huffman, i32) {
        let mut count = [0u16; MAXBITS + 1];
        for &l in lengths {
            count[l as usize] += 1;
        }
        let mut left: i32 = 1;
        if count[0] as usize == lengths.len() {
            return (
                Huffman {
                    count,
                    symbol: Vec::new(),
                },
                0,
            );
        }
        for len in 1..=MAXBITS {
            left <<= 1;
            left -= count[len] as i32;
            if left < 0 {
                return (
                    Huffman {
                        count,
                        symbol: Vec::new(),
                    },
                    left,
                );
            }
        }
        let mut offs = [0u16; MAXBITS + 1];
        for len in 1..MAXBITS {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        (Huffman { count, symbol }, left)
    }
}

const LBASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEXT: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
    window: Vec<u8>,
    wpos: usize,
    total_out: u64,
    max_out: u64,
    keep: usize,
    head: Vec<u8>,
    adler_a: u32,
    adler_b: u32,
}

impl Decoder<'_> {
    fn bits(&mut self, need: u32) -> Result<u32, Stop> {
        let mut val = self.bitbuf;
        while self.bitcnt < need {
            let b = *self.input.get(self.pos).ok_or(Stop::Data)? as u32;
            self.pos += 1;
            val |= b << self.bitcnt;
            self.bitcnt += 8;
        }
        self.bitbuf = if need >= 32 { 0 } else { val >> need };
        self.bitcnt -= need;
        Ok(val & ((1u32 << need) - 1))
    }

    fn decode(&mut self, h: &Huffman) -> Result<u16, Stop> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAXBITS {
            code |= self.bits(1)? as i32;
            let count = h.count[len] as i32;
            if code - count < first {
                return Ok(h.symbol[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(Stop::Data)
    }

    fn emit(&mut self, b: u8) -> Result<(), Stop> {
        if self.total_out >= self.max_out {
            return Err(Stop::Output);
        }
        self.window[self.wpos] = b;
        self.wpos = (self.wpos + 1) & (WINDOW - 1);
        if self.head.len() < self.keep {
            self.head.push(b);
        }
        self.total_out += 1;
        self.adler_a = (self.adler_a + b as u32) % 65521;
        self.adler_b = (self.adler_b + self.adler_a) % 65521;
        Ok(())
    }

    fn codes(&mut self, lencode: &Huffman, distcode: &Huffman) -> Result<(), Stop> {
        loop {
            let sym = self.decode(lencode)? as usize;
            if sym < 256 {
                self.emit(sym as u8)?;
            } else if sym == 256 {
                return Ok(());
            } else {
                let sym = sym - 257;
                if sym >= 29 {
                    return Err(Stop::Data);
                }
                let len = LBASE[sym] as usize + self.bits(LEXT[sym] as u32)? as usize;
                let dsym = self.decode(distcode)? as usize;
                if dsym >= 30 {
                    return Err(Stop::Data);
                }
                let dist = DBASE[dsym] as u64 + self.bits(DEXT[dsym] as u32)? as u64;
                if dist > self.total_out {
                    return Err(Stop::Data);
                }
                for _ in 0..len {
                    let b = self.window[(self.wpos + WINDOW - dist as usize) & (WINDOW - 1)];
                    self.emit(b)?;
                }
            }
        }
    }

    fn stored(&mut self) -> Result<(), Stop> {
        self.bitbuf = 0;
        self.bitcnt = 0;
        let hdr = self.input.get(self.pos..self.pos + 4).ok_or(Stop::Data)?;
        let len = hdr[0] as usize | (hdr[1] as usize) << 8;
        let nlen = hdr[2] as usize | (hdr[3] as usize) << 8;
        if len != (!nlen & 0xffff) {
            return Err(Stop::Data);
        }
        self.pos += 4;
        for _ in 0..len {
            let b = *self.input.get(self.pos).ok_or(Stop::Data)?;
            self.pos += 1;
            self.emit(b)?;
        }
        Ok(())
    }

    fn fixed(&mut self) -> Result<(), Stop> {
        let mut lengths = [0u8; 288];
        for (i, l) in lengths.iter_mut().enumerate() {
            *l = match i {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
        }
        let (lencode, _) = Huffman::new(&lengths);
        let (distcode, _) = Huffman::new(&[5u8; 30]);
        self.codes(&lencode, &distcode)
    }

    fn dynamic(&mut self) -> Result<(), Stop> {
        const ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        let nlen = self.bits(5)? as usize + 257;
        let ndist = self.bits(5)? as usize + 1;
        let ncode = self.bits(4)? as usize + 4;
        if nlen > 286 || ndist > 30 {
            return Err(Stop::Data);
        }
        let mut lengths = [0u8; 320];
        for &o in ORDER.iter().take(ncode) {
            lengths[o] = self.bits(3)? as u8;
        }
        let (lencode, err) = Huffman::new(&lengths[..19]);
        if err != 0 {
            return Err(Stop::Data);
        }
        let mut lengths = [0u8; 320];
        let mut index = 0usize;
        while index < nlen + ndist {
            let sym = self.decode(&lencode)? as usize;
            if sym < 16 {
                lengths[index] = sym as u8;
                index += 1;
            } else {
                let (len, rep) = match sym {
                    16 => {
                        if index == 0 {
                            return Err(Stop::Data);
                        }
                        (lengths[index - 1], 3 + self.bits(2)? as usize)
                    }
                    17 => (0, 3 + self.bits(3)? as usize),
                    _ => (0, 11 + self.bits(7)? as usize),
                };
                if index + rep > nlen + ndist {
                    return Err(Stop::Data);
                }
                for _ in 0..rep {
                    lengths[index] = len;
                    index += 1;
                }
            }
        }
        if lengths[256] == 0 {
            return Err(Stop::Data);
        }
        let (lencode, err) = Huffman::new(&lengths[..nlen]);
        if err < 0 || (err > 0 && nlen != lencode.count[0] as usize + lencode.count[1] as usize) {
            return Err(Stop::Data);
        }
        let (distcode, err) = Huffman::new(&lengths[nlen..nlen + ndist]);
        if err < 0 || (err > 0 && ndist != distcode.count[0] as usize + distcode.count[1] as usize)
        {
            return Err(Stop::Data);
        }
        self.codes(&lencode, &distcode)
    }

    fn run(&mut self) -> Result<(), Stop> {
        // zlib header (RFC 1950).
        let cmf = *self.input.first().ok_or(Stop::Data)? as u32;
        let flg = *self.input.get(1).ok_or(Stop::Data)? as u32;
        self.pos = 2;
        #[allow(clippy::manual_is_multiple_of)] // is_multiple_of needs Rust 1.87
        let bad_check = (cmf * 256 + flg) % 31 != 0;
        if cmf & 0x0f != 8 || cmf >> 4 > 7 || bad_check || flg & 0x20 != 0 {
            return Err(Stop::Data);
        }
        loop {
            let last = self.bits(1)?;
            match self.bits(2)? {
                0 => self.stored()?,
                1 => self.fixed()?,
                2 => self.dynamic()?,
                _ => return Err(Stop::Data),
            }
            if last == 1 {
                break;
            }
        }
        // Adler-32 trailer (byte aligned).
        self.bitbuf = 0;
        self.bitcnt = 0;
        let t = self.input.get(self.pos..self.pos + 4).ok_or(Stop::Data)?;
        let want = u32::from_be_bytes([t[0], t[1], t[2], t[3]]);
        if want != (self.adler_b << 16 | self.adler_a) {
            return Err(Stop::Data);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `zlib.compress(b"hello hello hello hello")`: one fixed-Huffman block.
    const FIXED: [u8; 16] = [
        120, 156, 203, 72, 205, 201, 201, 87, 200, 64, 39, 1, 104, 3, 8, 177,
    ];

    /// `zlib.compress(PLAIN, 9)`: one dynamic-Huffman block.
    const PLAIN: &[u8] = b"bab aacabdacaaabbaaacbadaa  daddbaaacabbacadbc aadd abacadad";
    const DYNAMIC: [u8; 44] = [
        120, 218, 29, 138, 65, 13, 0, 48, 12, 132, 172, 212, 26, 119, 248, 215, 176, 174, 31, 18,
        8, 33, 3, 37, 46, 32, 89, 52, 8, 51, 162, 231, 63, 23, 211, 93, 117, 56, 195, 7, 150, 40,
        21, 175,
    ];

    fn stored_stream(data: &[u8]) -> Vec<u8> {
        let mut v = vec![0x78, 0x01, 0x01];
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(&(!(data.len() as u16)).to_le_bytes());
        v.extend_from_slice(data);
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        v.extend_from_slice(&(b << 16 | a).to_be_bytes());
        v
    }

    #[test]
    fn inflates_fixed_block_with_match() {
        let out = zlib_inflate(&FIXED, 1000, 100);
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.head, b"hello hello hello hello");
        assert_eq!(out.total_out, 23);
    }

    #[test]
    fn inflates_dynamic_block() {
        let out = zlib_inflate(&DYNAMIC, 1000, 1000);
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.head, PLAIN);
    }

    #[test]
    fn inflates_stored_block() {
        let out = zlib_inflate(&stored_stream(b"abcdefghij"), 10, 4);
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.head, b"abcd");
        assert_eq!(out.total_out, 10);
    }

    #[test]
    fn output_limit_and_truncation() {
        let out = zlib_inflate(&FIXED, 5, 64);
        assert_eq!(out.status, Status::BufError);
        assert_eq!(out.head, b"hello");
        let out = zlib_inflate(&FIXED[..FIXED.len() - 6], 100, 64);
        assert_eq!(out.status, Status::DataError);
        let mut bad = FIXED;
        bad[15] ^= 1;
        assert_eq!(zlib_inflate(&bad, 100, 64).status, Status::DataError);
        assert_eq!(zlib_inflate(&[], 100, 64).status, Status::DataError);
        assert_eq!(
            zlib_inflate(&[0x78, 0x9d], 100, 64).status,
            Status::DataError
        );
    }
}
