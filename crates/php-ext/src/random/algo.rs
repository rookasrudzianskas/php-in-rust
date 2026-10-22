//! The generators of ext/random: Mt19937 (`engine_mt19937.c`), PCG
//! oneseq-128-xsl-rr-64 (`engine_pcgoneseq128xslrr64.c`), xoshiro256**
//! (`engine_xoshiro256starstar.c`), the combined LCG and the CSPRNG
//! (`csprng.c`). Everything here is independent of the VM.

use std::cell::RefCell;

use php_runtime::{PhpArray, PhpStr, Value};

/// `PHP_MT_RAND_MAX`.
pub const PHP_MT_RAND_MAX: i64 = 0x7FFF_FFFF;
/// `PHP_RANDOM_RANGE_ATTEMPTS`.
pub const RANGE_ATTEMPTS: u32 = 50;
/// `MT_RAND_MT19937`.
pub const MT_RAND_MT19937: u32 = 0;
/// `MT_RAND_PHP`.
pub const MT_RAND_PHP: u32 = 1;

/// `php_random_result`: up to 64 bits of output and how many bytes of it are meaningful.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RandResult {
    pub result: u64,
    pub size: usize,
}

// ---------------------------------------------------------------------------
// Hex helpers (php_random_bin2hex_le / php_random_hex2bin_le)
// ---------------------------------------------------------------------------

/// Little-endian bytes as lowercase hex.
pub fn bin2hex_le(bytes: &[u8]) -> Value {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize]);
        out.push(DIGITS[(b & 15) as usize]);
    }
    Value::String(PhpStr::from_vec(out))
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Parses `2 * N` hex digits into `N` bytes; `None` if invalid.
pub fn hex2bin_le<const N: usize>(hex: &[u8]) -> Option<[u8; N]> {
    if hex.len() != 2 * N {
        return None;
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = (hex_val(hex[2 * i])? << 4) | hex_val(hex[2 * i + 1])?;
    }
    Some(out)
}

/// `t = zend_hash_index_find(data, i)` requiring a string of exactly `N` bytes (hex).
fn state_hex<const N: usize>(data: &PhpArray, i: i64) -> Option<[u8; N]> {
    match data.get_int(i)? {
        Value::String(s) => hex2bin_le::<N>(s.as_bytes()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Mt19937
// ---------------------------------------------------------------------------

const MT_N: usize = 624;
const MT_M: usize = 397;

/// `php_random_status_state_mt19937`.
#[derive(Clone)]
pub struct Mt19937 {
    pub count: u32,
    pub mode: u32,
    pub state: [u32; MT_N],
}

impl Default for Mt19937 {
    fn default() -> Self {
        Mt19937 {
            count: 0,
            mode: MT_RAND_MT19937,
            state: [0; MT_N],
        }
    }
}

#[inline]
fn mix_bits(u: u32, v: u32) -> u32 {
    (u & 0x8000_0000) | (v & 0x7FFF_FFFF)
}

/// `twist(m, u, v)`.
#[inline]
fn twist(m: u32, u: u32, v: u32) -> u32 {
    m ^ (mix_bits(u, v) >> 1) ^ ((v & 1).wrapping_neg() & 0x9908_b0df)
}

/// `twist_php(m, u, v)`: the broken MT_RAND_PHP variant looks at the low bit of `u`.
#[inline]
fn twist_php(m: u32, u: u32, v: u32) -> u32 {
    m ^ (mix_bits(u, v) >> 1) ^ ((u & 1).wrapping_neg() & 0x9908_b0df)
}

impl Mt19937 {
    fn reload(&mut self) {
        let tw: fn(u32, u32, u32) -> u32 = if self.mode == MT_RAND_MT19937 {
            twist
        } else {
            twist_php
        };
        let s = &mut self.state;
        let mut p = 0usize;
        for _ in 0..(MT_N - MT_M) {
            s[p] = tw(s[p + MT_M], s[p], s[p + 1]);
            p += 1;
        }
        for _ in 1..MT_M {
            s[p] = tw(s[p + MT_M - MT_N], s[p], s[p + 1]);
            p += 1;
        }
        s[p] = tw(s[p + MT_M - MT_N], s[p], s[0]);
        self.count = 0;
    }

    /// `php_random_mt19937_seed32`.
    pub fn seed32(&mut self, seed: u32) {
        self.state[0] = seed;
        for i in 1..MT_N {
            let prev = self.state[i - 1];
            self.state[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.count = MT_N as u32;
        self.reload();
    }

    /// `generate()`: 32 bits of output.
    pub fn generate(&mut self) -> RandResult {
        if self.count as usize >= MT_N {
            self.reload();
        }
        let mut s1 = self.state[self.count as usize];
        self.count += 1;
        s1 ^= s1 >> 11;
        s1 ^= (s1 << 7) & 0x9d2c_5680;
        s1 ^= (s1 << 15) & 0xefc6_0000;
        RandResult {
            result: (s1 ^ (s1 >> 18)) as u64,
            size: 4,
        }
    }

    /// `serialize`: 624 hex words, the count and the mode.
    pub fn serialize(&self) -> PhpArray {
        let mut a = PhpArray::new();
        for w in &self.state {
            let _ = a.append(bin2hex_le(&w.to_le_bytes()));
        }
        let _ = a.append(Value::Int(self.count as i64));
        let _ = a.append(Value::Int(self.mode as i64));
        a
    }

    /// `unserialize` (updates in place, like php-src, so a failure part-way
    /// leaves the earlier words overwritten).
    pub fn unserialize(&mut self, data: &PhpArray) -> bool {
        if data.len() != MT_N + 2 {
            return false;
        }
        for i in 0..MT_N {
            match state_hex::<4>(data, i as i64) {
                Some(b) => self.state[i] = u32::from_le_bytes(b),
                None => return false,
            }
        }
        let Some(Value::Int(count)) = data.get_int(MT_N as i64) else {
            return false;
        };
        self.count = *count as u32;
        if self.count > MT_N as u32 {
            return false;
        }
        let Some(Value::Int(mode)) = data.get_int(MT_N as i64 + 1) else {
            return false;
        };
        self.mode = *mode as u32;
        self.mode == MT_RAND_MT19937 || self.mode == MT_RAND_PHP
    }
}

// ---------------------------------------------------------------------------
// PCG oneseq 128 XSL RR 64
// ---------------------------------------------------------------------------

const PCG_MULT: u128 = ((2_549_297_995_355_413_924u128) << 64) + 4_865_540_595_714_422_341u128;
const PCG_PLUS: u128 = ((6_364_136_223_846_793_005u128) << 64) + 1_442_695_040_888_963_407u128;

/// `php_random_status_state_pcgoneseq128xslrr64`.
#[derive(Clone, Copy, Default)]
pub struct Pcg128 {
    pub state: u128,
}

impl Pcg128 {
    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(PCG_PLUS);
    }

    /// `php_random_pcgoneseq128xslrr64_seed128`.
    pub fn seed128(&mut self, seed: u128) {
        self.state = 0;
        self.step();
        self.state = self.state.wrapping_add(seed);
        self.step();
    }

    pub fn generate(&mut self) -> RandResult {
        self.step();
        let hi = (self.state >> 64) as u64;
        let lo = self.state as u64;
        let s = (self.state >> 122) as u32;
        RandResult {
            result: (hi ^ lo).rotate_right(s),
            size: 8,
        }
    }

    /// `php_random_pcgoneseq128xslrr64_advance`.
    pub fn advance(&mut self, mut advance: u64) {
        let mut cur_mult = PCG_MULT;
        let mut cur_plus = PCG_PLUS;
        let mut acc_mult: u128 = 1;
        let mut acc_plus: u128 = 0;
        while advance > 0 {
            if advance & 1 == 1 {
                acc_mult = acc_mult.wrapping_mul(cur_mult);
                acc_plus = acc_plus.wrapping_mul(cur_mult).wrapping_add(cur_plus);
            }
            cur_plus = cur_mult.wrapping_add(1).wrapping_mul(cur_plus);
            cur_mult = cur_mult.wrapping_mul(cur_mult);
            advance /= 2;
        }
        self.state = acc_mult.wrapping_mul(self.state).wrapping_add(acc_plus);
    }

    pub fn serialize(&self) -> PhpArray {
        let mut a = PhpArray::new();
        let _ = a.append(bin2hex_le(&((self.state >> 64) as u64).to_le_bytes()));
        let _ = a.append(bin2hex_le(&(self.state as u64).to_le_bytes()));
        a
    }

    pub fn unserialize(&mut self, data: &PhpArray) -> bool {
        if data.len() != 2 {
            return false;
        }
        let mut u = [0u64; 2];
        for (i, slot) in u.iter_mut().enumerate() {
            match state_hex::<8>(data, i as i64) {
                Some(b) => *slot = u64::from_le_bytes(b),
                None => return false,
            }
        }
        self.state = ((u[0] as u128) << 64) | u[1] as u128;
        true
    }
}

// ---------------------------------------------------------------------------
// xoshiro256**
// ---------------------------------------------------------------------------

fn splitmix64(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut r = *seed;
    r = (r ^ (r >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    r = (r ^ (r >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    r ^ (r >> 31)
}

/// `php_random_status_state_xoshiro256starstar`.
#[derive(Clone, Copy, Default)]
pub struct Xoshiro256 {
    pub state: [u64; 4],
}

impl Xoshiro256 {
    fn generate_state(&mut self) -> u64 {
        let s = &mut self.state;
        let r = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        r
    }

    fn jump_with(&mut self, jmp: &[u64; 4]) {
        let (mut s0, mut s1, mut s2, mut s3) = (0u64, 0u64, 0u64, 0u64);
        for word in jmp {
            for j in 0..64 {
                if word & (1u64 << j) != 0 {
                    s0 ^= self.state[0];
                    s1 ^= self.state[1];
                    s2 ^= self.state[2];
                    s3 ^= self.state[3];
                }
                self.generate_state();
            }
        }
        self.state = [s0, s1, s2, s3];
    }

    pub fn seed256(&mut self, s0: u64, s1: u64, s2: u64, s3: u64) {
        self.state = [s0, s1, s2, s3];
    }

    /// `php_random_xoshiro256starstar_seed64`.
    pub fn seed64(&mut self, mut seed: u64) {
        let s0 = splitmix64(&mut seed);
        let s1 = splitmix64(&mut seed);
        let s2 = splitmix64(&mut seed);
        let s3 = splitmix64(&mut seed);
        self.seed256(s0, s1, s2, s3);
    }

    pub fn generate(&mut self) -> RandResult {
        RandResult {
            result: self.generate_state(),
            size: 8,
        }
    }

    pub fn jump(&mut self) {
        self.jump_with(&[
            0x180e_c6d3_3cfd_0aba,
            0xd5a6_1266_f0c9_392c,
            0xa958_2618_e03f_c9aa,
            0x39ab_dc45_29b1_661c,
        ]);
    }

    pub fn jump_long(&mut self) {
        self.jump_with(&[
            0x76e1_5d3e_fefd_cbbf,
            0xc500_4e44_1c52_2fb3,
            0x7771_0069_854e_e241,
            0x3910_9bb0_2acb_e635,
        ]);
    }

    pub fn serialize(&self) -> PhpArray {
        let mut a = PhpArray::new();
        for w in &self.state {
            let _ = a.append(bin2hex_le(&w.to_le_bytes()));
        }
        a
    }

    pub fn unserialize(&mut self, data: &PhpArray) -> bool {
        if data.len() != 4 {
            return false;
        }
        for i in 0..4 {
            match state_hex::<8>(data, i as i64) {
                Some(b) => self.state[i] = u64::from_le_bytes(b),
                None => return false,
            }
        }
        self.state.iter().any(|w| *w != 0)
    }
}

// ---------------------------------------------------------------------------
// Combined LCG
// ---------------------------------------------------------------------------

/// The body of `php_combined_lcg` after seeding: advances `state` and returns
/// a float in (0, 1).
pub fn combined_lcg_step(state: &mut [i32; 2]) -> f64 {
    fn modmult(a: i32, b: i32, c: i32, m: i32, s: &mut i32) {
        let q = *s / a;
        *s = b
            .wrapping_mul(s.wrapping_sub(a.wrapping_mul(q)))
            .wrapping_sub(c.wrapping_mul(q));
        if *s < 0 {
            *s = s.wrapping_add(m);
        }
    }
    modmult(53668, 40014, 12211, 2_147_483_563, &mut state[0]);
    modmult(52774, 40692, 3791, 2_147_483_399, &mut state[1]);
    let mut z = state[0].wrapping_sub(state[1]);
    if z < 1 {
        z = z.wrapping_add(2_147_483_562);
    }
    (z as i64 as u64) as f64 * 4.656613e-10
}

// ---------------------------------------------------------------------------
// CSPRNG and fallback seeds
// ---------------------------------------------------------------------------

thread_local! {
    static URANDOM: RefCell<Option<std::fs::File>> = const { RefCell::new(None) };
}

/// `php_random_bytes_ex`: fills `buf` from the operating system; the error is
/// the message php-src would put in the `RandomException`.
pub fn random_bytes(buf: &mut [u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Read;
        URANDOM.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                match std::fs::File::open("/dev/urandom") {
                    Ok(f) => *slot = Some(f),
                    Err(e) => return Err(format!("Cannot open /dev/urandom: {e}")),
                }
            }
            let f = slot.as_mut().expect("opened above");
            match f.read_exact(buf) {
                Ok(()) => Ok(()),
                Err(e) => Err(format!("Could not gather sufficient random data: {e}")),
            }
        })
    }
    #[cfg(not(unix))]
    {
        // No OS interface in std: use the randomly keyed SipHash of `RandomState`.
        use std::hash::{BuildHasher, Hasher};
        let _ = &URANDOM;
        let state = std::collections::hash_map::RandomState::new();
        let mut counter = 0u64;
        for chunk in buf.chunks_mut(8) {
            let mut h = state.build_hasher();
            h.write_u64(counter);
            counter += 1;
            let bytes = h.finish().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        Ok(())
    }
}

/// `php_random_int`: a uniformly distributed integer in `min..=max` from the CSPRNG.
pub fn random_int(min: i64, max: i64) -> Result<i64, String> {
    if min == max {
        return Ok(min);
    }
    let mut umax = (max as u64).wrapping_sub(min as u64);
    let next = || -> Result<u64, String> {
        let mut b = [0u8; 8];
        random_bytes(&mut b)?;
        Ok(u64::from_le_bytes(b))
    };
    let mut trial = next()?;
    if umax == u64::MAX {
        return Ok(trial as i64);
    }
    umax += 1;
    if umax & (umax - 1) != 0 {
        let limit = u64::MAX - (u64::MAX % umax) - 1;
        while trial > limit {
            trial = next()?;
        }
    }
    Ok((trial % umax).wrapping_add(min as u64) as i64)
}

/// `php_random_fallback_seed_state`.
#[derive(Default, Clone)]
pub struct FallbackSeedState {
    initialized: bool,
    seed: [u8; 20],
}

/// `php_random_generate_fallback_seed_ex`: mixes time, process ids, addresses
/// and (when available) OS randomness into 160 bits and returns the first 64.
/// php-src uses SHA-1 as the mixing function; the exact function is not
/// observable, only that the result is unpredictable and not cryptographically
/// safe.
pub fn generate_fallback_seed(state: &mut FallbackSeedState) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64 ^ d.as_secs().rotate_left(32))
    };
    let mut acc: [u64; 3] = [
        0x243f_6a88_85a3_08d3,
        0x1319_8a2e_0370_7344,
        0xa409_3822_299f_31d0,
    ];
    let feed = |v: u64, acc: &mut [u64; 3]| {
        let mut s = acc[0] ^ v;
        for slot in acc.iter_mut() {
            *slot ^= splitmix64(&mut s);
        }
    };
    feed(now(), &mut acc);
    if state.initialized {
        for chunk in state.seed.chunks(8) {
            let mut b = [0u8; 8];
            b[..chunk.len()].copy_from_slice(chunk);
            feed(u64::from_le_bytes(b), &mut acc);
        }
    } else {
        feed(std::process::id() as u64, &mut acc);
        let local = 0u8;
        feed(&local as *const u8 as usize as u64, &mut acc);
        feed(&acc as *const _ as usize as u64, &mut acc);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&std::thread::current().id(), &mut h);
        feed(std::hash::Hasher::finish(&h), &mut acc);
        let mut buf = [0u8; 16];
        if random_bytes(&mut buf).is_ok() {
            feed(
                u64::from_le_bytes(buf[..8].try_into().expect("8 bytes")),
                &mut acc,
            );
            feed(
                u64::from_le_bytes(buf[8..].try_into().expect("8 bytes")),
                &mut acc,
            );
        }
        feed(now(), &mut acc);
    }
    let mut seed = [0u8; 20];
    seed[..8].copy_from_slice(&acc[0].to_le_bytes());
    seed[8..16].copy_from_slice(&acc[1].to_le_bytes());
    seed[16..].copy_from_slice(&acc[2].to_le_bytes()[..4]);
    state.seed = seed;
    state.initialized = true;
    u64::from_le_bytes(seed[..8].try_into().expect("8 bytes"))
}

/// The per-request globals of ext/random (`ZEND_BEGIN_MODULE_GLOBALS(random)`).
#[derive(Default)]
pub struct RandomGlobals {
    pub combined_lcg_seeded: bool,
    pub mt19937_seeded: bool,
    pub fallback_seed_state: FallbackSeedState,
    pub combined_lcg: [i32; 2],
    pub mt19937: Mt19937,
}

impl RandomGlobals {
    /// `php_random_default_status`: the global Mt19937, seeded on first use.
    pub fn default_mt(&mut self) -> &mut Mt19937 {
        if !self.mt19937_seeded {
            self.mt19937.mode = MT_RAND_MT19937;
            self.seed_mt_default();
            self.mt19937_seeded = true;
        }
        &mut self.mt19937
    }

    /// `php_random_mt19937_seed_default` on the global state.
    pub fn seed_mt_default(&mut self) {
        seed_mt_default(&mut self.mt19937, &mut self.fallback_seed_state);
    }

    /// `php_combined_lcg`.
    pub fn combined_lcg(&mut self) -> f64 {
        if !self.combined_lcg_seeded {
            let mut b = [0u8; 8];
            let seed = if random_bytes(&mut b).is_ok() {
                u64::from_le_bytes(b)
            } else {
                generate_fallback_seed(&mut self.fallback_seed_state)
            };
            self.combined_lcg[0] = (seed & 0xffff_ffff) as u32 as i32;
            self.combined_lcg[1] = (seed >> 32) as u32 as i32;
            self.combined_lcg_seeded = true;
        }
        combined_lcg_step(&mut self.combined_lcg)
    }
}

/// `php_random_mt19937_seed_default`: seeds from the CSPRNG, silently falling
/// back to the fallback seed.
pub fn seed_mt_default(state: &mut Mt19937, fallback: &mut FallbackSeedState) {
    let mut b = [0u8; 4];
    let seed = if random_bytes(&mut b).is_ok() {
        u32::from_le_bytes(b)
    } else {
        generate_fallback_seed(fallback) as u32
    };
    state.seed32(seed);
}
