//! Argon2i / Argon2id (RFC 9106) with the semantics of the libargon2 library
//! php-src links against (`HAVE_ARGON2LIB`): input validation, error
//! messages, the `$argon2i$v=19$m=..,t=..,p=..$salt$hash` encoding and
//! `argon2_verify()`.
//!
//! Argon2d is implemented as well (it shares the core) but PHP never exposes
//! it; it is used by the RFC test vectors.

use super::blake2b::{Blake2b, OUT_BYTES, blake2b};

pub const VERSION_10: u32 = 0x10;
pub const VERSION_13: u32 = 0x13;

/// `ARGON2_SYNC_POINTS`: slices per pass.
const SYNC_POINTS: u32 = 4;
/// Number of 64-bit words in a 1 KiB block.
const BLOCK_WORDS: usize = 128;
const BLOCK_BYTES: usize = BLOCK_WORDS * 8;
/// `ARGON2_PREHASH_DIGEST_LENGTH`.
const PREHASH_DIGEST_LENGTH: usize = 64;
/// `ARGON2_PREHASH_SEED_LENGTH`.
const PREHASH_SEED_LENGTH: usize = PREHASH_DIGEST_LENGTH + 8;

/// `ARGON2_MIN_MEMORY`.
pub const MIN_MEMORY: u64 = 2 * SYNC_POINTS as u64;
/// `ARGON2_MAX_MEMORY`.
pub const MAX_MEMORY: u64 = u32::MAX as u64;
/// `ARGON2_MIN_TIME`.
pub const MIN_TIME: u64 = 1;
/// `ARGON2_MAX_TIME`.
pub const MAX_TIME: u64 = u32::MAX as u64;
/// `ARGON2_MAX_LANES` (and `ARGON2_MAX_THREADS`).
pub const MAX_LANES: u64 = 0xFF_FFFF;
const MIN_OUTLEN: usize = 4;
const MIN_SALT_LENGTH: usize = 8;
const MAX_LENGTH: u64 = u32::MAX as u64;

/// `argon2_type`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Type {
    /// Only used by the RFC 9106 test vectors; PHP does not expose Argon2d.
    #[cfg_attr(not(test), allow(dead_code))]
    D = 0,
    I = 1,
    Id = 2,
}

impl Type {
    /// `argon2_type2string(type, 0)`.
    fn name(self) -> &'static str {
        match self {
            Type::D => "argon2d",
            Type::I => "argon2i",
            Type::Id => "argon2id",
        }
    }
}

/// The `argon2_error_codes` this implementation can produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    OutputTooShort,
    PwdTooLong,
    SaltTooShort,
    SaltTooLong,
    SecretTooLong,
    AdTooLong,
    TimeTooSmall,
    TimeTooLarge,
    MemoryTooLittle,
    MemoryTooMuch,
    LanesTooFew,
    LanesTooMany,
    MemoryAllocation,
    DecodingFail,
    VerifyMismatch,
}

impl Error {
    /// `argon2_error_message()`.
    pub fn message(self) -> &'static str {
        match self {
            Error::OutputTooShort => "Output is too short",
            Error::PwdTooLong => "Password is too long",
            Error::SaltTooShort => "Salt is too short",
            Error::SaltTooLong => "Salt is too long",
            Error::SecretTooLong => "Secret is too long",
            Error::AdTooLong => "Associated data is too long",
            Error::TimeTooSmall => "Time cost is too small",
            Error::TimeTooLarge => "Time cost is too large",
            Error::MemoryTooLittle => "Memory cost is too small",
            Error::MemoryTooMuch => "Memory cost is too large",
            Error::LanesTooFew => "Too few lanes",
            Error::LanesTooMany => "Too many lanes",
            Error::MemoryAllocation => "Memory allocation error",
            Error::DecodingFail => "Decoding failed",
            Error::VerifyMismatch => "The password does not match the supplied hash",
        }
    }
}

/// `argon2_context` (without the callbacks and flags).
pub struct Params<'a> {
    pub t_cost: u32,
    pub m_cost: u32,
    pub lanes: u32,
    pub pwd: &'a [u8],
    pub salt: &'a [u8],
    pub secret: &'a [u8],
    pub ad: &'a [u8],
    pub outlen: u32,
    pub version: u32,
}

/// `validate_inputs()` (threads always equal lanes).
fn validate_inputs(p: &Params) -> Result<(), Error> {
    // Validate output length.
    if (MIN_OUTLEN as u64) > p.outlen as u64 {
        return Err(Error::OutputTooShort);
    }
    // Validate password (required param).
    if p.pwd.len() as u64 > MAX_LENGTH {
        return Err(Error::PwdTooLong);
    }
    // Validate salt (required param).
    if p.salt.len() < MIN_SALT_LENGTH {
        return Err(Error::SaltTooShort);
    }
    if p.salt.len() as u64 > MAX_LENGTH {
        return Err(Error::SaltTooLong);
    }
    // Validate secret and associated data (optional params).
    if p.secret.len() as u64 > MAX_LENGTH {
        return Err(Error::SecretTooLong);
    }
    if p.ad.len() as u64 > MAX_LENGTH {
        return Err(Error::AdTooLong);
    }
    // Validate memory cost.
    if MIN_MEMORY > p.m_cost as u64 {
        return Err(Error::MemoryTooLittle);
    }
    if (p.m_cost as u64) < 8 * p.lanes as u64 {
        return Err(Error::MemoryTooLittle);
    }
    if MAX_MEMORY < p.m_cost as u64 {
        return Err(Error::MemoryTooMuch);
    }
    // Validate time cost.
    if MIN_TIME > p.t_cost as u64 {
        return Err(Error::TimeTooSmall);
    }
    if MAX_TIME < p.t_cost as u64 {
        return Err(Error::TimeTooLarge);
    }
    // Validate lanes and threads.
    if p.lanes < 1 {
        return Err(Error::LanesTooFew);
    }
    if MAX_LANES < p.lanes as u64 {
        return Err(Error::LanesTooMany);
    }
    Ok(())
}

/// `blake2b_long()`: the variable-length hash H'.
fn blake2b_long(outlen: usize, input: &[u8]) -> Vec<u8> {
    let outlen_bytes = (outlen as u32).to_le_bytes();
    if outlen <= OUT_BYTES {
        let mut b = Blake2b::new(outlen);
        b.update(&outlen_bytes);
        b.update(input);
        return b.finalize();
    }

    let mut out = Vec::with_capacity(outlen);
    let mut b = Blake2b::new(OUT_BYTES);
    b.update(&outlen_bytes);
    b.update(input);
    let mut out_buffer = b.finalize();
    out.extend_from_slice(&out_buffer[..OUT_BYTES / 2]);
    let mut toproduce = outlen - OUT_BYTES / 2;

    while toproduce > OUT_BYTES {
        out_buffer = blake2b(OUT_BYTES, &out_buffer);
        out.extend_from_slice(&out_buffer[..OUT_BYTES / 2]);
        toproduce -= OUT_BYTES / 2;
    }

    out_buffer = blake2b(toproduce, &out_buffer);
    out.extend_from_slice(&out_buffer[..toproduce]);
    out
}

/// `initial_hash()`: H0.
fn initial_hash(ty: Type, p: &Params) -> Vec<u8> {
    let mut h = Blake2b::new(PREHASH_DIGEST_LENGTH);
    h.update(&p.lanes.to_le_bytes());
    h.update(&p.outlen.to_le_bytes());
    h.update(&p.m_cost.to_le_bytes());
    h.update(&p.t_cost.to_le_bytes());
    h.update(&p.version.to_le_bytes());
    h.update(&(ty as u32).to_le_bytes());
    h.update(&(p.pwd.len() as u32).to_le_bytes());
    h.update(p.pwd);
    h.update(&(p.salt.len() as u32).to_le_bytes());
    h.update(p.salt);
    h.update(&(p.secret.len() as u32).to_le_bytes());
    h.update(p.secret);
    h.update(&(p.ad.len() as u32).to_le_bytes());
    h.update(p.ad);
    h.finalize()
}

type Block = [u64; BLOCK_WORDS];

/// `fBlaMka()`: the multiplication-hardened addition.
#[inline(always)]
fn fb_la_mka(x: u64, y: u64) -> u64 {
    const M: u64 = 0xFFFF_FFFF;
    let xy = (x & M).wrapping_mul(y & M);
    x.wrapping_add(y).wrapping_add(xy.wrapping_mul(2))
}

/// Argon2's `G` (BLAKE2b's with `fBlaMka` instead of plain addition).
#[inline(always)]
fn g(v: &mut Block, a: usize, b: usize, c: usize, d: usize) {
    v[a] = fb_la_mka(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = fb_la_mka(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = fb_la_mka(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = fb_la_mka(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

/// `BLAKE2_ROUND_NOMSG` over the sixteen words at `i`.
#[inline(always)]
fn blake2_round_nomsg(v: &mut Block, i: [usize; 16]) {
    g(v, i[0], i[4], i[8], i[12]);
    g(v, i[1], i[5], i[9], i[13]);
    g(v, i[2], i[6], i[10], i[14]);
    g(v, i[3], i[7], i[11], i[15]);
    g(v, i[0], i[5], i[10], i[15]);
    g(v, i[1], i[6], i[11], i[12]);
    g(v, i[2], i[7], i[8], i[13]);
    g(v, i[3], i[4], i[9], i[14]);
}

/// `fill_block()`: `next = P(prev ^ ref) ^ prev ^ ref` (XORed into the old
/// `next` when `with_xor`).
fn fill_block(prev: &Block, reference: &Block, next: &mut Block, with_xor: bool) {
    let mut block_r = [0u64; BLOCK_WORDS];
    for i in 0..BLOCK_WORDS {
        block_r[i] = reference[i] ^ prev[i];
    }
    let mut block_tmp = block_r;
    if with_xor {
        for i in 0..BLOCK_WORDS {
            block_tmp[i] ^= next[i];
        }
    }

    // Blake2 on columns of 64-bit words: (0,1,...,15), then (16,17,..31)...
    for i in 0..8 {
        let b = 16 * i;
        blake2_round_nomsg(
            &mut block_r,
            [
                b,
                b + 1,
                b + 2,
                b + 3,
                b + 4,
                b + 5,
                b + 6,
                b + 7,
                b + 8,
                b + 9,
                b + 10,
                b + 11,
                b + 12,
                b + 13,
                b + 14,
                b + 15,
            ],
        );
    }
    // Blake2 on rows of 64-bit words: (0,1,16,17,...,112,113), then
    // (2,3,18,19,...,114,115)...
    for i in 0..8 {
        let b = 2 * i;
        blake2_round_nomsg(
            &mut block_r,
            [
                b,
                b + 1,
                b + 16,
                b + 17,
                b + 32,
                b + 33,
                b + 48,
                b + 49,
                b + 64,
                b + 65,
                b + 80,
                b + 81,
                b + 96,
                b + 97,
                b + 112,
                b + 113,
            ],
        );
    }

    for i in 0..BLOCK_WORDS {
        next[i] = block_tmp[i] ^ block_r[i];
    }
}

/// `next_addresses()`: the next block of data-independent addresses.
fn next_addresses(address_block: &mut Block, input_block: &mut Block, zero_block: &Block) {
    input_block[6] = input_block[6].wrapping_add(1);
    fill_block(zero_block, input_block, address_block, false);
    let a = *address_block;
    fill_block(zero_block, &a, address_block, false);
}

/// `argon2_instance_t`.
struct Instance {
    memory: Vec<u64>,
    version: u32,
    passes: u32,
    memory_blocks: u32,
    segment_length: u32,
    lane_length: u32,
    lanes: u32,
    ty: Type,
}

impl Instance {
    fn block(&self, index: usize) -> Block {
        let mut b = [0u64; BLOCK_WORDS];
        b.copy_from_slice(&self.memory[index * BLOCK_WORDS..(index + 1) * BLOCK_WORDS]);
        b
    }

    fn store_block(&mut self, index: usize, block: &Block) {
        self.memory[index * BLOCK_WORDS..(index + 1) * BLOCK_WORDS].copy_from_slice(block);
    }

    /// `index_alpha()`: the index of the reference block inside the lane.
    fn index_alpha(
        &self,
        pass: u32,
        slice: u32,
        index: u32,
        pseudo_rand: u32,
        same_lane: bool,
    ) -> u32 {
        // Pass 0: this lane: all finished segments plus the already
        // constructed blocks of this segment; other lanes: all finished
        // segments. Later passes: the last (SYNC_POINTS - 1) segments plus the
        // constructed blocks of this segment (this lane) or just the segments.
        let reference_area_size: u32 = if pass == 0 {
            if slice == 0 {
                // All but the previous block.
                index.wrapping_sub(1)
            } else if same_lane {
                (slice * self.segment_length + index).wrapping_sub(1)
            } else {
                (slice * self.segment_length).wrapping_sub((index == 0) as u32)
            }
        } else if same_lane {
            (self.lane_length - self.segment_length + index).wrapping_sub(1)
        } else {
            (self.lane_length - self.segment_length).wrapping_sub((index == 0) as u32)
        };

        // Map pseudo_rand to 0..<reference_area_size-1>.
        let mut relative_position = pseudo_rand as u64;
        relative_position = (relative_position * relative_position) >> 32;
        let ras = reference_area_size as u64;
        relative_position = (ras.wrapping_sub(1)).wrapping_sub((ras * relative_position) >> 32);

        // Starting position.
        let start_position: u64 = if pass != 0 {
            if slice == SYNC_POINTS - 1 {
                0
            } else {
                (slice as u64 + 1) * self.segment_length as u64
            }
        } else {
            0
        };

        ((start_position + relative_position) % self.lane_length as u64) as u32
    }

    /// `fill_segment()` (the reference implementation's).
    fn fill_segment(&mut self, pass: u32, lane: u32, slice: u32) {
        let data_independent_addressing =
            self.ty == Type::I || (self.ty == Type::Id && pass == 0 && slice < SYNC_POINTS / 2);

        let zero_block = [0u64; BLOCK_WORDS];
        let mut input_block = [0u64; BLOCK_WORDS];
        let mut address_block = [0u64; BLOCK_WORDS];
        if data_independent_addressing {
            input_block[0] = pass as u64;
            input_block[1] = lane as u64;
            input_block[2] = slice as u64;
            input_block[3] = self.memory_blocks as u64;
            input_block[4] = self.passes as u64;
            input_block[5] = self.ty as u64;
        }

        let mut starting_index = 0u32;
        if pass == 0 && slice == 0 {
            // The first two blocks of each lane are already generated.
            starting_index = 2;
            // Don't forget to generate the first block of addresses.
            if data_independent_addressing {
                next_addresses(&mut address_block, &mut input_block, &zero_block);
            }
        }

        let lane_length = self.lane_length as usize;
        let lane_start = lane as usize * lane_length;

        for i in starting_index..self.segment_length {
            // Position inside the lane and offsets of the current and the
            // previous block (the lane's last block precedes its first).
            let col = slice as usize * self.segment_length as usize + i as usize;
            let curr_offset = lane_start + col;
            let prev_offset = if col == 0 {
                lane_start + lane_length - 1
            } else {
                curr_offset - 1
            };

            // The pseudo-random value comes from the address block or from
            // the previous block.
            let pseudo_rand: u64 = if data_independent_addressing {
                if i as usize & (BLOCK_WORDS - 1) == 0 {
                    next_addresses(&mut address_block, &mut input_block, &zero_block);
                }
                address_block[i as usize & (BLOCK_WORDS - 1)]
            } else {
                self.memory[prev_offset * BLOCK_WORDS]
            };

            // The lane of the reference block.
            let mut ref_lane = ((pseudo_rand >> 32) % self.lanes as u64) as u32;
            if pass == 0 && slice == 0 {
                // Can not reference other lanes yet.
                ref_lane = lane;
            }

            let ref_index = self.index_alpha(
                pass,
                slice,
                i,
                (pseudo_rand & 0xFFFF_FFFF) as u32,
                ref_lane == lane,
            );

            // Creating the new block.
            let prev = self.block(prev_offset);
            let reference = self.block(ref_lane as usize * lane_length + ref_index as usize);
            let mut next = self.block(curr_offset);
            let with_xor = self.version != VERSION_10 && pass != 0;
            fill_block(&prev, &reference, &mut next, with_xor);
            self.store_block(curr_offset, &next);
        }
    }
}

/// `argon2_ctx()`: the raw tag.
pub fn hash_raw(ty: Type, p: &Params) -> Result<Vec<u8>, Error> {
    validate_inputs(p)?;

    // Align the memory size: at least 2 * SYNC_POINTS blocks per lane, and
    // all segments have equal length.
    let mut memory_blocks = p.m_cost;
    if memory_blocks < 2 * SYNC_POINTS * p.lanes {
        memory_blocks = 2 * SYNC_POINTS * p.lanes;
    }
    let segment_length = memory_blocks / (p.lanes * SYNC_POINTS);
    let memory_blocks = segment_length * (p.lanes * SYNC_POINTS);

    let words = memory_blocks as usize * BLOCK_WORDS;
    let mut memory: Vec<u64> = Vec::new();
    memory
        .try_reserve_exact(words)
        .map_err(|_| Error::MemoryAllocation)?;
    memory.resize(words, 0);

    let mut instance = Instance {
        memory,
        version: p.version,
        passes: p.t_cost,
        memory_blocks,
        segment_length,
        lane_length: segment_length * SYNC_POINTS,
        lanes: p.lanes,
        ty,
    };

    // Initialization: H0, then the first two blocks of every lane.
    let blockhash = initial_hash(ty, p);
    let mut seed = [0u8; PREHASH_SEED_LENGTH];
    seed[..PREHASH_DIGEST_LENGTH].copy_from_slice(&blockhash);
    for lane in 0..p.lanes {
        for col in 0..2u32 {
            seed[PREHASH_DIGEST_LENGTH..PREHASH_DIGEST_LENGTH + 4]
                .copy_from_slice(&col.to_le_bytes());
            seed[PREHASH_DIGEST_LENGTH + 4..].copy_from_slice(&lane.to_le_bytes());
            let bytes = blake2b_long(BLOCK_BYTES, &seed);
            let mut block = [0u64; BLOCK_WORDS];
            for (w, chunk) in block.iter_mut().zip(bytes.chunks(8)) {
                let mut b = [0u8; 8];
                b.copy_from_slice(chunk);
                *w = u64::from_le_bytes(b);
            }
            let index = lane as usize * instance.lane_length as usize + col as usize;
            instance.store_block(index, &block);
        }
    }

    // Filling the memory, segment by segment (lanes of one slice are
    // independent of each other).
    for pass in 0..p.t_cost {
        for slice in 0..SYNC_POINTS {
            for lane in 0..p.lanes {
                instance.fill_segment(pass, lane, slice);
            }
        }
    }

    // Finalization: XOR of the last block of every lane, hashed down to the
    // tag length.
    let last_in_lane =
        |l: u32| (l as usize * instance.lane_length as usize) + instance.lane_length as usize - 1;
    let mut blockhash = instance.block(last_in_lane(0));
    for l in 1..p.lanes {
        let b = instance.block(last_in_lane(l));
        for (x, y) in blockhash.iter_mut().zip(b.iter()) {
            *x ^= *y;
        }
    }
    let mut blockhash_bytes = Vec::with_capacity(BLOCK_BYTES);
    for w in blockhash {
        blockhash_bytes.extend_from_slice(&w.to_le_bytes());
    }
    Ok(blake2b_long(p.outlen as usize, &blockhash_bytes))
}

// ---------------------------------------------------------------------------
// Encoding (encoding.c)
// ---------------------------------------------------------------------------

/// Unpadded standard base64 (`to_base64()`).
fn to_base64(src: &[u8]) -> Vec<u8> {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(src.len().div_ceil(3) * 4);
    for chunk in src.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63]);
        out.push(B64[(n >> 12) as usize & 63]);
        if chunk.len() > 1 {
            out.push(B64[(n >> 6) as usize & 63]);
        }
        if chunk.len() > 2 {
            out.push(B64[n as usize & 63]);
        }
    }
    out
}

/// `from_base64()`: decodes the leading run of base64 characters of `src`
/// and returns the bytes and the index of the first non-base64 character;
/// `None` for invalid trailing bits.
fn from_base64(src: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut acc_len: u32 = 0;
    let mut pos = 0;
    while let Some(&c) = src.get(pos) {
        let d = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => break,
        } as u32;
        pos += 1;
        acc = (acc << 6).wrapping_add(d);
        acc_len += 6;
        if acc_len >= 8 {
            acc_len -= 8;
            out.push(((acc >> acc_len) & 0xFF) as u8);
        }
    }

    // If the input length is equal to 1 modulo 4 (which is invalid), 6 bits
    // remain unprocessed; otherwise only 0, 2 or 4 bits are buffered, and
    // they must all be zero.
    if acc_len > 4 || (acc & ((1u32 << acc_len) - 1)) != 0 {
        return None;
    }
    Some((out, pos))
}

/// `encode_string()` for the raw tag `hash`.
pub fn encode(ty: Type, p: &Params, hash: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(b"$");
    s.extend_from_slice(ty.name().as_bytes());
    s.extend_from_slice(format!("$v={}", p.version).as_bytes());
    s.extend_from_slice(format!("$m={},t={},p={}$", p.m_cost, p.t_cost, p.lanes).as_bytes());
    s.extend_from_slice(&to_base64(p.salt));
    s.push(b'$');
    s.extend_from_slice(&to_base64(hash));
    s
}

/// `decode_decimal()`: a decimal without leading zeros; the value and the
/// index after the digits.
fn decode_decimal(s: &[u8]) -> Option<(u64, usize)> {
    let mut acc: u64 = 0;
    let mut i = 0;
    while let Some(&c) = s.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        let c = (c - b'0') as u64;
        if acc > u64::MAX / 10 {
            return None;
        }
        acc *= 10;
        if c > u64::MAX - acc {
            return None;
        }
        acc += c;
        i += 1;
    }
    if i == 0 || (s[0] == b'0' && i != 1) {
        return None;
    }
    Some((acc, i))
}

/// The fields `decode_string()` fills into the context.
struct Decoded {
    version: u32,
    m_cost: u32,
    t_cost: u32,
    lanes: u32,
    salt: Vec<u8>,
    out: Vec<u8>,
}

/// `decode_string()`; `encoded` is the C string's contents.
fn decode(encoded: &[u8], ty: Type, pwd: &[u8]) -> Result<Decoded, Error> {
    let mut s = encoded;

    fn expect<'a>(s: &'a [u8], prefix: &str) -> Result<&'a [u8], Error> {
        s.strip_prefix(prefix.as_bytes()).ok_or(Error::DecodingFail)
    }
    fn decimal_u32(s: &[u8]) -> Result<(u32, &[u8]), Error> {
        match decode_decimal(s) {
            Some((v, n)) if v <= u32::MAX as u64 => Ok((v as u32, &s[n..])),
            _ => Err(Error::DecodingFail),
        }
    }

    s = expect(s, "$")?;
    s = expect(s, ty.name())?;

    // Reading the version number if the default is suppressed.
    let mut version = VERSION_10;
    if let Some(rest) = s.strip_prefix(b"$v=") {
        let (v, rest) = decimal_u32(rest)?;
        version = v;
        s = rest;
    }

    s = expect(s, "$m=")?;
    let (m_cost, rest) = decimal_u32(s)?;
    s = expect(rest, ",t=")?;
    let (t_cost, rest) = decimal_u32(s)?;
    s = expect(rest, ",p=")?;
    let (lanes, rest) = decimal_u32(s)?;
    s = rest;

    // Reading the salt and the hash.
    s = expect(s, "$")?;
    let (salt, n) = from_base64(s).ok_or(Error::DecodingFail)?;
    s = expect(&s[n..], "$")?;
    let (out, n) = from_base64(s).ok_or(Error::DecodingFail)?;
    s = &s[n..];

    // The rest of the input string should be empty.
    if !s.is_empty() {
        return Err(Error::DecodingFail);
    }

    let d = Decoded {
        version,
        m_cost,
        t_cost,
        lanes,
        salt,
        out,
    };
    validate_inputs(&Params {
        t_cost: d.t_cost,
        m_cost: d.m_cost,
        lanes: d.lanes,
        pwd,
        salt: &d.salt,
        secret: &[],
        ad: &[],
        outlen: d.out.len().min(u32::MAX as usize) as u32,
        version: d.version,
    })?;
    Ok(d)
}

// ---------------------------------------------------------------------------
// Public API (argon2.c)
// ---------------------------------------------------------------------------

/// `argon2_hash()` with encoding: the `$argon2..$` string for the raw `hash`
/// of `outlen` bytes.
pub fn hash_encoded(
    ty: Type,
    t_cost: u32,
    m_cost: u32,
    lanes: u32,
    pwd: &[u8],
    salt: &[u8],
    outlen: u32,
) -> Result<Vec<u8>, Error> {
    if pwd.len() as u64 > MAX_LENGTH {
        return Err(Error::PwdTooLong);
    }
    if salt.len() as u64 > MAX_LENGTH {
        return Err(Error::SaltTooLong);
    }
    if (outlen as usize) < MIN_OUTLEN {
        return Err(Error::OutputTooShort);
    }
    let params = Params {
        t_cost,
        m_cost,
        lanes,
        pwd,
        salt,
        secret: &[],
        ad: &[],
        outlen,
        version: VERSION_13,
    };
    let raw = hash_raw(ty, &params)?;
    Ok(encode(ty, &params, &raw))
}

/// `argon2_verify()`: `Ok(())` if `pwd` hashes to the `encoded` tag.
pub fn verify(encoded: &[u8], pwd: &[u8], ty: Type) -> Result<(), Error> {
    if pwd.len() as u64 > MAX_LENGTH {
        return Err(Error::PwdTooLong);
    }
    let d = decode(encoded, ty, pwd)?;
    let params = Params {
        t_cost: d.t_cost,
        m_cost: d.m_cost,
        lanes: d.lanes,
        pwd,
        salt: &d.salt,
        secret: &[],
        ad: &[],
        outlen: d.out.len() as u32,
        version: d.version,
    };
    let computed = hash_raw(ty, &params)?;
    // argon2_compare(): constant time.
    let mut diff = 0u8;
    for (a, b) in computed.iter().zip(&d.out) {
        diff |= std::hint::black_box(a ^ b);
    }
    if diff != 0 || computed.len() != d.out.len() {
        return Err(Error::VerifyMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// RFC 9106 section 5: password 32x01, salt 16x02, secret 8x03, associated
    /// data 12x04, t = 3, m = 32 KiB, p = 4, 32 byte tag, version 0x13.
    fn rfc(ty: Type) -> String {
        let pwd = [1u8; 32];
        let salt = [2u8; 16];
        let secret = [3u8; 8];
        let ad = [4u8; 12];
        let p = Params {
            t_cost: 3,
            m_cost: 32,
            lanes: 4,
            pwd: &pwd,
            salt: &salt,
            secret: &secret,
            ad: &ad,
            outlen: 32,
            version: VERSION_13,
        };
        hex(&hash_raw(ty, &p).unwrap())
    }

    #[test]
    fn rfc9106_argon2d() {
        assert_eq!(
            rfc(Type::D),
            "512b391b6f1162975371d30919734294f868e3be3984f3c1a13a4db9fabe4acb"
        );
    }

    #[test]
    fn rfc9106_argon2i() {
        assert_eq!(
            rfc(Type::I),
            "c814d9d1dc7f37aa13f0d77f2494bda1c8de6b016dd388d29952a4c4672b6ce8"
        );
    }

    #[test]
    fn rfc9106_argon2id() {
        assert_eq!(
            rfc(Type::Id),
            "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659"
        );
    }

    #[test]
    fn reference_implementation_vectors() {
        // phc-winner-argon2 src/test.c: password / somesalt, t=2, m=2^16 KiB, p=1.
        let hashtest = |version: u32, t: u32, m: u32, p: u32, hash: &str| {
            let params = Params {
                t_cost: t,
                m_cost: m,
                lanes: p,
                pwd: b"password",
                salt: b"somesalt",
                secret: &[],
                ad: &[],
                outlen: 32,
                version,
            };
            assert_eq!(hex(&hash_raw(Type::I, &params).unwrap()), hash);
        };
        hashtest(
            VERSION_10,
            2,
            1 << 16,
            1,
            "f6c4db4a54e2a370627aff3db6176b94a2a209a62c8e36152711802f7b30c694",
        );
        hashtest(
            VERSION_13,
            2,
            1 << 16,
            1,
            "c1628832147d9720c5bd1cfd61367078729f6dfb6f8fea9ff98158e0d7816ed0",
        );
    }

    #[test]
    fn readme_cli_vector() {
        // `echo -n password | argon2 somesalt -t 2 -m 16 -p 4 -l 24`
        let enc = hash_encoded(Type::I, 2, 1 << 16, 4, b"password", b"somesalt", 24).unwrap();
        assert_eq!(
            String::from_utf8(enc).unwrap(),
            "$argon2i$v=19$m=65536,t=2,p=4$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG"
        );
    }

    /// ext/standard/tests/password/password_verify_argon2.phpt (hashes made by
    /// a real libargon2).
    #[test]
    fn php_argon2_hashes_verify() {
        let argon2i = b"$argon2i$v=19$m=65536,t=3,p=1$OEVjWWs2Z3YvWlNZQ0ZmNw$JKin7ahjmh8JYvMyFcXri0Ss/Uvd3uYpD7MG6C/5Cy0";
        assert_eq!(verify(argon2i, b"test", Type::I), Ok(()));
        assert_eq!(
            verify(argon2i, b"argon2", Type::I),
            Err(Error::VerifyMismatch)
        );
        let argon2id = b"$argon2id$v=19$m=1024,t=2,p=2$WS90MHJhd3AwSC5xTDJpZg$8tn2DaIJR2/UX4Cjcy2t3EZaLDL/qh+NbLQAOvTmdAg";
        assert_eq!(verify(argon2id, b"test", Type::Id), Ok(()));
        assert_eq!(
            verify(argon2id, b"argon2id", Type::Id),
            Err(Error::VerifyMismatch)
        );
        // The type prefix must match.
        assert_eq!(verify(argon2id, b"test", Type::I), Err(Error::DecodingFail));
        assert_eq!(verify(argon2i, b"test", Type::Id), Err(Error::DecodingFail));
    }

    #[test]
    fn hash_and_verify_roundtrip() {
        for ty in [Type::I, Type::Id] {
            let enc = hash_encoded(
                ty,
                2,
                64,
                2,
                b"the password for testing 12345!",
                b"0123456789abcdef",
                32,
            )
            .unwrap();
            assert!(enc.starts_with(format!("${}$v=19$m=64,t=2,p=2$", ty.name()).as_bytes()));
            assert_eq!(verify(&enc, b"the password for testing 12345!", ty), Ok(()));
            assert_eq!(verify(&enc, b"other", ty), Err(Error::VerifyMismatch));
        }
    }

    #[test]
    fn parameter_validation() {
        let h = |t, m, p, salt: &[u8], outlen| hash_encoded(Type::I, t, m, p, b"pw", salt, outlen);
        assert_eq!(h(1, 7, 1, b"saltsalt", 32), Err(Error::MemoryTooLittle));
        assert_eq!(h(1, 8, 2, b"saltsalt", 32), Err(Error::MemoryTooLittle));
        assert_eq!(h(0, 8, 1, b"saltsalt", 32), Err(Error::TimeTooSmall));
        assert_eq!(h(1, 8, 0, b"saltsalt", 32), Err(Error::LanesTooFew));
        assert_eq!(h(1, 8, 1, b"salt", 32), Err(Error::SaltTooShort));
        assert_eq!(h(1, 8, 1, b"saltsalt", 3), Err(Error::OutputTooShort));
        assert_eq!(
            h(1, 8, 0x100_0000, b"saltsalt", 32),
            Err(Error::MemoryTooLittle)
        );
        // Smallest legal memory.
        assert!(h(1, 8, 1, b"saltsalt", 32).is_ok());
    }

    #[test]
    fn decoding_rules() {
        let ok = b"$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG";
        // Sanity: parses (mismatch, not a decoding error).
        assert_eq!(verify(ok, b"x", Type::I), Err(Error::VerifyMismatch));
        for bad in [
            &b"$argon2i$v=19$m=08,t=1,p=1$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG"[..],
            b"$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ=$RdescudvJCsgt3ub+b+dWRWJTmaaJObG",
            b"$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG$",
            b"$argon2i$v=19$m=8,t=1$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG",
            b"$argon2i$v=19$m=4294967296,t=1,p=1$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG",
            b"$argon2i$m=8,t=1,p=1$c29tZXNhbHQ$Rdescu",
            b"argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG",
            b"",
        ] {
            assert!(
                matches!(
                    verify(bad, b"x", Type::I),
                    Err(Error::DecodingFail) | Err(Error::OutputTooShort)
                ),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        // Invalid trailing bits in the salt: "c29tZXNhbHR" has dangling bits.
        assert_eq!(
            verify(
                b"$argon2i$v=19$m=8,t=1,p=1$c29tZXNhbHS$RdescudvJCsgt3ub+b+dWRWJTmaaJObG",
                b"x",
                Type::I
            ),
            Err(Error::DecodingFail)
        );
        // Salt shorter than 8 bytes.
        assert_eq!(
            verify(
                b"$argon2i$v=19$m=8,t=1,p=1$c29tZQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG",
                b"x",
                Type::I
            ),
            Err(Error::SaltTooShort)
        );
        // Zero lanes.
        assert_eq!(
            verify(
                b"$argon2i$v=19$m=8,t=1,p=0$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG",
                b"x",
                Type::I
            ),
            Err(Error::LanesTooFew)
        );
    }

    #[test]
    fn blake2b_long_lengths() {
        assert_eq!(blake2b_long(32, b"abc").len(), 32);
        assert_eq!(blake2b_long(64, b"abc").len(), 64);
        assert_eq!(blake2b_long(65, b"abc").len(), 65);
        assert_eq!(blake2b_long(1024, b"abc").len(), 1024);
        // Up to 64 bytes it is plain BLAKE2b over LE32(len) || input.
        let mut input = 40u32.to_le_bytes().to_vec();
        input.extend_from_slice(b"abc");
        assert_eq!(blake2b_long(40, b"abc"), blake2b(40, &input));
    }
}
