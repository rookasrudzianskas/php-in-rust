//! Pure-Rust implementations of every hash algorithm of PHP's `ext/hash` (and of
//! `md5()`, `sha1()` and `crc32()` from `ext/standard`), byte-identical to PHP.
//!
//! The crate offers the building blocks the PHP functions `hash()`,
//! `hash_algos()`, `hash_hmac_algos()`, `hash_hmac()`,
//! `hash_init()`/`hash_update()`/`hash_final()`/`hash_copy()`,
//! `HashContext::__serialize()`/`__unserialize()`, `hash_pbkdf2()`,
//! `hash_hkdf()`, `md5()`, `sha1()` and `crc32()` are made of; the PHP-level
//! functions themselves (argument parsing, warnings, files and streams) live
//! elsewhere.
//!
//! * [`algorithms`], [`hash_algos`], [`hash_hmac_algos`] and [`find`] expose the
//!   registry in the order PHP reports it ([`HashAlgorithm`]: name, digest size,
//!   block size, cryptographic flag).
//! * [`new_hasher`] creates an incremental, cloneable [`Hasher`]
//!   (`hash_init`/`hash_update`/`hash_final`/`hash_copy`); [`hash`] and
//!   [`hash_with_options`] are the one-shot forms.  [`HashOptions`] carries the
//!   `seed` and `secret` options of the murmur3 and xxh families.
//! * [`HashContext`] wraps a hasher that may carry an HMAC key
//!   (`hash_init($algo, HASH_HMAC, $key)`); [`Hmac`], [`hmac`], [`pbkdf2`] and
//!   [`hkdf`] implement the keyed constructions with PHP's argument semantics.
//! * [`Hasher::serialize_state`] / [`Hasher::unserialize_state`] reproduce PHP's
//!   serialised context layout, including the error codes PHP reports.
//! * [`md5`], [`sha1`], [`crc32`] and [`to_hex`] are the `ext/standard` helpers.
//!
//! ```
//! use php_hash::{HashOptions, hash, new_hasher, to_hex};
//!
//! assert_eq!(
//!     to_hex(&hash("sha256", b"abc").unwrap()),
//!     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
//! );
//!
//! // hash_init() / hash_update() / hash_copy() / hash_final()
//! let mut ctx = new_hasher("crc32b", &HashOptions::none()).unwrap();
//! ctx.update(b"The quick brown fox ");
//! let mut copy = ctx.clone();
//! ctx.update(b"jumped over the lazy dog.");
//! copy.update(b"jumped over the lazy dog.");
//! assert_eq!(to_hex(&ctx.finalize()), "82a34642");
//! assert_eq!(to_hex(&copy.finalize()), "82a34642");
//!
//! // Seeded xxh64, as `hash('xxh64', 'x', options: ['seed' => 42])`.
//! let seeded = php_hash::hash_with_options("xxh64", b"x", &HashOptions::with_seed(42)).unwrap();
//! assert_eq!(seeded.len(), 8);
//! ```
//!
//! # Fidelity notes
//!
//! * Serialised contexts follow the layout of an x86-64 PHP build: the MD5
//!   scratch block is serialised as zeros, MurmurHash3 carries use the
//!   `UNALIGNED_SAFE` code path and SHA-3 uses the 64-bit Keccak
//!   lane-complementing state with magic `100`.  `xxh3` and `xxh128` cannot be
//!   serialised (as in PHP).
//! * The deprecated `mhash*()` compatibility functions are not covered.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod checksum;
mod gost;
mod hasher;
mod haval;
mod hmac;
mod md;
mod murmur;
mod registry;
mod ripemd;
mod sha1;
mod sha2;
mod sha3;
mod snefru;
mod standard;
mod state;
mod tables;
mod tiger;
mod whirlpool;
mod xxhash;

pub use checksum::crc32;
pub use hasher::{HashError, HashOptions, Hasher, to_hex};
pub use hmac::{HashContext, Hmac, hkdf, hmac, pbkdf2};
pub use registry::{
    HashAlgorithm, algorithms, find, hash, hash_algos, hash_hmac_algos, hash_with_options,
    new_hasher,
};
pub use standard::{md5, md5_hex, sha1, sha1_hex};
pub use state::{SERIALIZE_MAGIC_SPEC, StateError, StateItem};

/// Smallest accepted secret for `xxh3`/`xxh128` (`XXH3_SECRET_SIZE_MIN`).
pub const XXH3_SECRET_SIZE_MIN: usize = 136;
/// Largest secret `xxh3`/`xxh128` keep; longer ones are truncated (PHP warns).
pub const XXH3_SECRET_SIZE_MAX: usize = 256;
