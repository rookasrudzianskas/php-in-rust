//! The incremental hashing interface shared by every algorithm.

use std::fmt;

use crate::registry::HashAlgorithm;
use crate::state::{
    SERIALIZE_MAGIC_SPEC, StateError, StateItem, StateReader, StateResult, StateWriter, Visitor,
};

/// Options accepted by `hash_init()` / `hash()` (the `$options` array of PHP).
///
/// Only a few algorithms look at them: `murmur3a/c/f` and `xxh32/xxh64/xxh3/xxh128`
/// use `seed`, while `xxh3`/`xxh128` additionally understand `secret`.  Callers
/// are expected to emit PHP's deprecation diagnostics themselves and map a
/// non-integer `seed` to `Some(0)`, which is what PHP effectively does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashOptions {
    /// Seed (`"seed"` key); PHP casts the integer to the algorithm's seed width.
    pub seed: Option<i64>,
    /// Custom secret (`"secret"` key) for `xxh3` and `xxh128`.
    pub secret: Option<Vec<u8>>,
}

impl HashOptions {
    /// No options at all.
    pub fn none() -> Self {
        Self::default()
    }

    /// Only a seed.
    pub fn with_seed(seed: i64) -> Self {
        Self {
            seed: Some(seed),
            secret: None,
        }
    }

    /// Only a secret (xxh3 / xxh128).
    pub fn with_secret(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            seed: None,
            secret: Some(secret.into()),
        }
    }
}

/// Errors reported by the high level functions of this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HashError {
    /// The algorithm name is not registered ("must be a valid hashing algorithm").
    UnknownAlgorithm,
    /// The algorithm exists but is not cryptographic
    /// ("must be a valid cryptographic hashing algorithm").
    NotCryptographic,
    /// `hash_init()` with HMAC requested but an empty key.
    EmptyKey,
    /// xxh3/xxh128 given both a seed and a secret.
    SeedAndSecret {
        /// Algorithm name (`"xxh3"` or `"xxh128"`).
        algo: &'static str,
    },
    /// xxh3/xxh128 secret shorter than [`crate::XXH3_SECRET_SIZE_MIN`].
    SecretTooShort {
        /// Algorithm name (`"xxh3"` or `"xxh128"`).
        algo: &'static str,
        /// Length of the secret that was passed.
        len: usize,
    },
    /// Requested output length exceeds what the construction can deliver.
    LengthTooLarge {
        /// Largest permitted length.
        max: usize,
    },
    /// A required input (HKDF key material) was empty.
    EmptyInput,
    /// PBKDF2 needs at least one iteration.
    InvalidIterations,
    /// A serialised context could not be loaded (see [`StateError::code`]).
    InvalidState(StateError),
}

impl fmt::Display for HashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HashError::UnknownAlgorithm => f.write_str("must be a valid hashing algorithm"),
            HashError::NotCryptographic => {
                f.write_str("must be a valid cryptographic hashing algorithm")
            }
            HashError::EmptyKey => f.write_str("must not be empty when HMAC is requested"),
            HashError::SeedAndSecret { algo } => write!(
                f,
                "{algo}: Only one of seed or secret is to be passed for initialization"
            ),
            HashError::SecretTooShort { algo, len } => write!(
                f,
                "{algo}: Secret length must be >= {} bytes, {len} bytes passed",
                crate::XXH3_SECRET_SIZE_MIN
            ),
            HashError::LengthTooLarge { max } => {
                write!(f, "must be less than or equal to {max}")
            }
            HashError::EmptyInput => f.write_str("must not be empty"),
            HashError::InvalidIterations => f.write_str("must be greater than 0"),
            HashError::InvalidState(e) => {
                write!(
                    f,
                    "incomplete or ill-formed serialization data (code {})",
                    e.code()
                )
            }
        }
    }
}

impl std::error::Error for HashError {}

/// An incremental hash context (`hash_init()` / `hash_update()` / `hash_final()`).
pub trait Hasher: Send {
    /// Registry entry of the algorithm this context computes.
    fn algorithm(&self) -> &'static HashAlgorithm;

    /// Feeds more input into the context.
    fn update(&mut self, data: &[u8]);

    /// Consumes the context and returns the raw digest (`digest_size` bytes).
    fn finalize(self: Box<Self>) -> Vec<u8>;

    /// Duplicates the context (`hash_copy()`).
    fn box_clone(&self) -> Box<dyn Hasher>;

    /// Exports the context in PHP's `HashContext::__serialize` format.
    ///
    /// Returns `(magic, state)`; fails with [`StateError::Failure`] for
    /// algorithms PHP cannot serialise (`xxh3`, `xxh128`).
    fn serialize_state(&self) -> Result<(i64, Vec<StateItem>), StateError>;

    /// Imports a state exported by [`Hasher::serialize_state`] into a *freshly
    /// created* context of the same algorithm (`HashContext::__unserialize`).
    fn unserialize_state(&mut self, magic: i64, state: &[StateItem]) -> Result<(), StateError>;

    /// Digest size in bytes.
    fn digest_size(&self) -> usize {
        self.algorithm().digest_size
    }

    /// Block size in bytes (the HMAC block size).
    fn block_size(&self) -> usize {
        self.algorithm().block_size
    }
}

impl Clone for Box<dyn Hasher> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}

/// Crate-private description of one algorithm's mutable state.
pub(crate) trait Engine: Clone + Send + 'static {
    /// `magic` written by `serialize_state` (`None`: not serialisable).
    const MAGIC: Option<i64> = Some(SERIALIZE_MAGIC_SPEC);

    fn update(&mut self, data: &[u8]);

    fn finish(self) -> Vec<u8>;

    /// Visits the serialised fields in C struct order.
    fn visit(&mut self, _v: &mut dyn Visitor) -> StateResult {
        Err(StateError::Failure)
    }

    /// Sanity checks after an import (PHP's per-algorithm validation hooks).
    fn validate(&self) -> bool {
        true
    }
}

/// Binds an [`Engine`] to its registry entry and implements [`Hasher`].
#[derive(Clone)]
pub(crate) struct Wrap<E: Engine> {
    algo: &'static HashAlgorithm,
    engine: E,
}

impl<E: Engine> Wrap<E> {
    pub(crate) fn boxed(algo: &'static HashAlgorithm, engine: E) -> Box<dyn Hasher> {
        Box::new(Self { algo, engine })
    }
}

impl<E: Engine> Hasher for Wrap<E> {
    fn algorithm(&self) -> &'static HashAlgorithm {
        self.algo
    }

    fn update(&mut self, data: &[u8]) {
        self.engine.update(data);
    }

    fn finalize(self: Box<Self>) -> Vec<u8> {
        self.engine.finish()
    }

    fn box_clone(&self) -> Box<dyn Hasher> {
        Box::new(self.clone())
    }

    fn serialize_state(&self) -> Result<(i64, Vec<StateItem>), StateError> {
        let magic = E::MAGIC.ok_or(StateError::Failure)?;
        let mut writer = StateWriter::default();
        let mut copy = self.engine.clone();
        copy.visit(&mut writer)?;
        Ok((magic, writer.finish()))
    }

    fn unserialize_state(&mut self, magic: i64, state: &[StateItem]) -> Result<(), StateError> {
        if E::MAGIC != Some(magic) {
            return Err(StateError::Failure);
        }
        self.engine.visit(&mut StateReader::new(state))?;
        if self.engine.validate() {
            Ok(())
        } else {
            Err(StateError::Validation)
        }
    }
}

/// Merkle-Damgard style buffering exactly as PHP's C code does it (the
/// pending bytes live in `buf[..index]`; stale bytes beyond are preserved
/// because they are visible in serialised contexts).
pub(crate) fn rsa_update<const N: usize>(
    buf: &mut [u8; N],
    index: usize,
    input: &[u8],
    mut transform: impl FnMut(&[u8; N]),
) {
    let part_len = N - index;
    let mut i = 0;
    let mut index = index;
    if input.len() >= part_len {
        buf[index..].copy_from_slice(&input[..part_len]);
        transform(buf);
        i = part_len;
        while i + N <= input.len() {
            let block: &[u8; N] = input[i..i + N].try_into().expect("block length");
            transform(block);
            i += N;
        }
        index = 0;
    }
    let rest = &input[i..];
    buf[index..index + rest.len()].copy_from_slice(rest);
}

/// Zero padding with a leading 0x80 byte, long enough for every algorithm.
pub(crate) const PADDING: [u8; 128] = {
    let mut p = [0u8; 128];
    p[0] = 0x80;
    p
};

/// 32-bit Merkle-Damgard core with a 64-byte block and a 64-bit bit counter
/// (md4, sha1, sha224/256, ripemd128/160/256/320).
#[derive(Clone)]
pub(crate) struct Md32<const S: usize> {
    pub state: [u32; S],
    pub count: u64,
    pub buffer: [u8; 64],
}

pub(crate) type Transform32<const S: usize> = fn(&mut [u32; S], &[u8; 64]);

impl<const S: usize> Md32<S> {
    pub(crate) fn new(state: [u32; S]) -> Self {
        Self {
            state,
            count: 0,
            buffer: [0; 64],
        }
    }

    pub(crate) fn update(&mut self, input: &[u8], transform: Transform32<S>) {
        let index = ((self.count >> 3) & 0x3f) as usize;
        self.count = self.count.wrapping_add((input.len() as u64) << 3);
        let state = &mut self.state;
        rsa_update(&mut self.buffer, index, input, |b| transform(state, b));
    }

    /// Appends the standard padding and length (little or big endian).
    pub(crate) fn pad(&mut self, transform: Transform32<S>, big_endian: bool) {
        let bits = if big_endian {
            self.count.to_be_bytes()
        } else {
            self.count.to_le_bytes()
        };
        let index = ((self.count >> 3) & 0x3f) as usize;
        let pad_len = if index < 56 { 56 - index } else { 120 - index };
        self.update(&PADDING[..pad_len], transform);
        self.update(&bits, transform);
    }

    pub(crate) fn visit(&mut self, v: &mut dyn Visitor) -> StateResult {
        v.u32s(&mut self.state)?;
        v.count32(&mut self.count)?;
        v.bytes(&mut self.buffer)
    }
}

/// Reads `N` little-endian 32-bit words from the start of `bytes`.
pub(crate) fn load_le32<const N: usize>(bytes: &[u8]) -> [u32; N] {
    std::array::from_fn(|i| {
        u32::from_le_bytes([
            bytes[4 * i],
            bytes[4 * i + 1],
            bytes[4 * i + 2],
            bytes[4 * i + 3],
        ])
    })
}

/// Reads `N` big-endian 32-bit words from the start of `bytes`.
pub(crate) fn load_be32<const N: usize>(bytes: &[u8]) -> [u32; N] {
    std::array::from_fn(|i| {
        u32::from_be_bytes([
            bytes[4 * i],
            bytes[4 * i + 1],
            bytes[4 * i + 2],
            bytes[4 * i + 3],
        ])
    })
}

/// Reads `N` little-endian 64-bit words from the start of `bytes`.
pub(crate) fn load_le64<const N: usize>(bytes: &[u8]) -> [u64; N] {
    std::array::from_fn(|i| read_le64(bytes, i))
}

/// Reads the `i`-th little-endian 64-bit word of `bytes`.
pub(crate) fn read_le64(bytes: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(word8(bytes, i))
}

/// Reads `N` big-endian 64-bit words from the start of `bytes`.
pub(crate) fn load_be64<const N: usize>(bytes: &[u8]) -> [u64; N] {
    std::array::from_fn(|i| u64::from_be_bytes(word8(bytes, i)))
}

fn word8(bytes: &[u8], i: usize) -> [u8; 8] {
    [
        bytes[8 * i],
        bytes[8 * i + 1],
        bytes[8 * i + 2],
        bytes[8 * i + 3],
        bytes[8 * i + 4],
        bytes[8 * i + 5],
        bytes[8 * i + 6],
        bytes[8 * i + 7],
    ]
}

/// Serialises 32-bit words, little or big endian, truncated to `len` bytes.
pub(crate) fn encode32(words: &[u32], big_endian: bool, len: usize) -> Vec<u8> {
    let mut out: Vec<u8> = words
        .iter()
        .flat_map(|w| {
            if big_endian {
                w.to_be_bytes()
            } else {
                w.to_le_bytes()
            }
        })
        .collect();
    out.truncate(len);
    out
}

/// Lower-case hexadecimal encoding of `bytes` (PHP's `bin2hex`).
pub fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(char::from(HEX[usize::from(b >> 4)]));
        s.push(char::from(HEX[usize::from(b & 15)]));
    }
    s
}
