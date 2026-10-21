//! The algorithm registry, in the order PHP's `hash_algos()` reports it.

use std::fmt;

use crate::checksum::{Adler32, Crc32, CrcKind, Fnv32, Fnv64, Joaat};
use crate::gost::Gost;
use crate::hasher::{HashError, HashOptions, Hasher, Wrap};
use crate::haval::Haval;
use crate::md::{Md2, Md4, Md5};
use crate::murmur::{Murmur3a, Murmur3c, Murmur3f};
use crate::ripemd::{ripemd128, ripemd160, ripemd256, ripemd320};
use crate::sha1::Sha1;
use crate::sha2::{Sha256, Sha512};
use crate::sha3::Sha3;
use crate::snefru::Snefru;
use crate::tiger::Tiger;
use crate::whirlpool::Whirlpool;
use crate::xxhash::{Xxh3, Xxh32, Xxh64};

type Ctor = fn(&'static HashAlgorithm, &HashOptions) -> Result<Box<dyn Hasher>, HashError>;

/// Static description of one hash algorithm (`php_hash_ops` plus its registered name).
#[derive(Clone, Copy)]
pub struct HashAlgorithm {
    /// Name as registered and returned by `hash_algos()` (always lower case).
    pub name: &'static str,
    /// Name PHP writes into serialised `HashContext`s (`ops->algo`); differs
    /// from [`HashAlgorithm::name`] only for the `snefru256` alias.
    pub serialized_name: &'static str,
    /// Digest size in bytes.
    pub digest_size: usize,
    /// Block size in bytes (used by HMAC).
    pub block_size: usize,
    /// Cryptographic algorithms are the ones usable with HMAC, PBKDF2 and HKDF.
    pub is_crypto: bool,
    ctor: Ctor,
}

impl fmt::Debug for HashAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashAlgorithm")
            .field("name", &self.name)
            .field("digest_size", &self.digest_size)
            .field("block_size", &self.block_size)
            .field("is_crypto", &self.is_crypto)
            .finish()
    }
}

impl PartialEq for HashAlgorithm {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for HashAlgorithm {}

impl HashAlgorithm {
    /// Creates a fresh incremental context for this algorithm.
    pub fn new_hasher(&'static self, options: &HashOptions) -> Result<Box<dyn Hasher>, HashError> {
        (self.ctor)(self, options)
    }

    /// Looks an algorithm up by (ASCII case-insensitive) name.
    pub fn find(name: &str) -> Option<&'static HashAlgorithm> {
        find(name)
    }
}

macro_rules! algo {
    ($name:literal, $digest:expr, $block:expr, $crypto:expr, $ctor:expr) => {
        algo!($name, $name, $digest, $block, $crypto, $ctor)
    };
    ($name:literal, $ser:literal, $digest:expr, $block:expr, $crypto:expr, $ctor:expr) => {
        HashAlgorithm {
            name: $name,
            serialized_name: $ser,
            digest_size: $digest,
            block_size: $block,
            is_crypto: $crypto,
            ctor: $ctor,
        }
    };
}

/// Constructor for an algorithm whose engine needs no options.
macro_rules! plain {
    ($engine:expr) => {
        |algo, _opts| Ok(Wrap::boxed(algo, $engine))
    };
}

/// All algorithms in `hash_algos()` order.
static ALGORITHMS: &[HashAlgorithm] = &[
    algo!("md2", 16, 16, true, plain!(Md2::new())),
    algo!("md4", 16, 64, true, plain!(Md4::new())),
    algo!("md5", 16, 64, true, plain!(Md5::new())),
    algo!("sha1", 20, 64, true, plain!(Sha1::new())),
    algo!("sha224", 28, 64, true, plain!(Sha256::new_224())),
    algo!("sha256", 32, 64, true, plain!(Sha256::new_256())),
    algo!("sha384", 48, 128, true, plain!(Sha512::new_384())),
    algo!("sha512/224", 28, 128, true, plain!(Sha512::new_512_224())),
    algo!("sha512/256", 32, 128, true, plain!(Sha512::new_512_256())),
    algo!("sha512", 64, 128, true, plain!(Sha512::new_512())),
    algo!("sha3-224", 28, 144, true, plain!(Sha3::new(224))),
    algo!("sha3-256", 32, 136, true, plain!(Sha3::new(256))),
    algo!("sha3-384", 48, 104, true, plain!(Sha3::new(384))),
    algo!("sha3-512", 64, 72, true, plain!(Sha3::new(512))),
    algo!("ripemd128", 16, 64, true, plain!(ripemd128())),
    algo!("ripemd160", 20, 64, true, plain!(ripemd160())),
    algo!("ripemd256", 32, 64, true, plain!(ripemd256())),
    algo!("ripemd320", 40, 64, true, plain!(ripemd320())),
    algo!("whirlpool", 64, 64, true, plain!(Whirlpool::new())),
    algo!("tiger128,3", 16, 64, true, plain!(Tiger::new(3, 128))),
    algo!("tiger160,3", 20, 64, true, plain!(Tiger::new(3, 160))),
    algo!("tiger192,3", 24, 64, true, plain!(Tiger::new(3, 192))),
    algo!("tiger128,4", 16, 64, true, plain!(Tiger::new(4, 128))),
    algo!("tiger160,4", 20, 64, true, plain!(Tiger::new(4, 160))),
    algo!("tiger192,4", 24, 64, true, plain!(Tiger::new(4, 192))),
    algo!("snefru", 32, 32, true, plain!(Snefru::new())),
    algo!("snefru256", "snefru", 32, 32, true, plain!(Snefru::new())),
    algo!("gost", 32, 32, true, plain!(Gost::new(false))),
    algo!("gost-crypto", 32, 32, true, plain!(Gost::new(true))),
    algo!("adler32", 4, 4, false, plain!(Adler32::new())),
    algo!("crc32", 4, 4, false, plain!(Crc32::new(CrcKind::Bzip2))),
    algo!("crc32b", 4, 4, false, plain!(Crc32::new(CrcKind::Iso))),
    algo!(
        "crc32c",
        4,
        4,
        false,
        plain!(Crc32::new(CrcKind::Castagnoli))
    ),
    algo!("fnv132", 4, 4, false, plain!(Fnv32::new(false))),
    algo!("fnv1a32", 4, 4, false, plain!(Fnv32::new(true))),
    algo!("fnv164", 8, 4, false, plain!(Fnv64::new(false))),
    algo!("fnv1a64", 8, 4, false, plain!(Fnv64::new(true))),
    algo!("joaat", 4, 4, false, plain!(Joaat::new())),
    algo!("murmur3a", 4, 4, false, |algo, opts| Ok(Wrap::boxed(
        algo,
        Murmur3a::new(opts.seed)
    ))),
    algo!("murmur3c", 16, 4, false, |algo, opts| Ok(Wrap::boxed(
        algo,
        Murmur3c::new(opts.seed)
    ))),
    algo!("murmur3f", 16, 8, false, |algo, opts| Ok(Wrap::boxed(
        algo,
        Murmur3f::new(opts.seed)
    ))),
    algo!("xxh32", 4, 4, false, |algo, opts| Ok(Wrap::boxed(
        algo,
        Xxh32::new(opts.seed)
    ))),
    algo!("xxh64", 8, 8, false, |algo, opts| Ok(Wrap::boxed(
        algo,
        Xxh64::new(opts.seed)
    ))),
    algo!("xxh3", 8, 8, false, |algo, opts| {
        Ok(Wrap::boxed(
            algo,
            Xxh3::new("xxh3", false, opts.seed, opts.secret.as_deref())?,
        ))
    }),
    algo!("xxh128", 16, 8, false, |algo, opts| {
        Ok(Wrap::boxed(
            algo,
            Xxh3::new("xxh128", true, opts.seed, opts.secret.as_deref())?,
        ))
    }),
    algo!("haval128,3", 16, 128, true, plain!(Haval::new(3, 128))),
    algo!("haval160,3", 20, 128, true, plain!(Haval::new(3, 160))),
    algo!("haval192,3", 24, 128, true, plain!(Haval::new(3, 192))),
    algo!("haval224,3", 28, 128, true, plain!(Haval::new(3, 224))),
    algo!("haval256,3", 32, 128, true, plain!(Haval::new(3, 256))),
    algo!("haval128,4", 16, 128, true, plain!(Haval::new(4, 128))),
    algo!("haval160,4", 20, 128, true, plain!(Haval::new(4, 160))),
    algo!("haval192,4", 24, 128, true, plain!(Haval::new(4, 192))),
    algo!("haval224,4", 28, 128, true, plain!(Haval::new(4, 224))),
    algo!("haval256,4", 32, 128, true, plain!(Haval::new(4, 256))),
    algo!("haval128,5", 16, 128, true, plain!(Haval::new(5, 128))),
    algo!("haval160,5", 20, 128, true, plain!(Haval::new(5, 160))),
    algo!("haval192,5", 24, 128, true, plain!(Haval::new(5, 192))),
    algo!("haval224,5", 28, 128, true, plain!(Haval::new(5, 224))),
    algo!("haval256,5", 32, 128, true, plain!(Haval::new(5, 256))),
];

/// Every registered algorithm, in `hash_algos()` order.
pub fn algorithms() -> &'static [HashAlgorithm] {
    ALGORITHMS
}

/// Names returned by PHP's `hash_algos()`.
pub fn hash_algos() -> Vec<&'static str> {
    ALGORITHMS.iter().map(|a| a.name).collect()
}

/// Names returned by PHP's `hash_hmac_algos()` (cryptographic algorithms only).
pub fn hash_hmac_algos() -> Vec<&'static str> {
    ALGORITHMS
        .iter()
        .filter(|a| a.is_crypto)
        .map(|a| a.name)
        .collect()
}

/// Finds an algorithm by name; the lookup lower-cases ASCII like PHP does.
pub fn find(name: &str) -> Option<&'static HashAlgorithm> {
    ALGORITHMS
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(name))
}

/// Creates a hashing context (`hash_init()` without HMAC).
pub fn new_hasher(name: &str, options: &HashOptions) -> Result<Box<dyn Hasher>, HashError> {
    find(name)
        .ok_or(HashError::UnknownAlgorithm)?
        .new_hasher(options)
}

/// One-shot hash of `data` with default options (`hash($algo, $data, true)`).
pub fn hash(name: &str, data: &[u8]) -> Result<Vec<u8>, HashError> {
    hash_with_options(name, data, &HashOptions::none())
}

/// One-shot hash of `data` with the `$options` array of `hash()`.
pub fn hash_with_options(
    name: &str,
    data: &[u8],
    options: &HashOptions,
) -> Result<Vec<u8>, HashError> {
    let mut hasher = new_hasher(name, options)?;
    hasher.update(data);
    Ok(hasher.finalize())
}
