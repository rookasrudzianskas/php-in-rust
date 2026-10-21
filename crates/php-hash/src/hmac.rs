//! HMAC, PBKDF2 and HKDF on top of the algorithm registry, plus the
//! `hash_init()` context that optionally carries an HMAC key.

use crate::hasher::{HashError, HashOptions, Hasher};
use crate::registry::{HashAlgorithm, find};
use crate::state::{StateError, StateItem};

fn crypto_algorithm(name: &str) -> Result<&'static HashAlgorithm, HashError> {
    match find(name) {
        Some(a) if a.is_crypto => Ok(a),
        _ => Err(HashError::NotCryptographic),
    }
}

/// Key schedule shared by every HMAC flavour: the key, zero padded (or hashed
/// first when longer than a block), XORed with `0x36` (inner) / `0x5c` (outer).
fn pads(algo: &'static HashAlgorithm, options: &HashOptions, key: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let block = algo.block_size;
    let mut k = vec![0u8; block];
    if key.len() > block {
        let mut h = algo
            .new_hasher(options)
            .expect("cryptographic algorithms accept default options");
        h.update(key);
        let digest = h.finalize();
        k[..digest.len()].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let ipad = k.iter().map(|b| b ^ 0x36).collect();
    let opad = k.iter().map(|b| b ^ 0x5c).collect();
    (ipad, opad)
}

/// Incremental HMAC (RFC 2104) over any cryptographic algorithm of the registry.
#[derive(Clone)]
pub struct Hmac {
    algo: &'static HashAlgorithm,
    inner: Box<dyn Hasher>,
    outer: Box<dyn Hasher>,
}

impl Hmac {
    /// Starts an HMAC; an empty key is allowed (as for `hash_hmac()`).
    ///
    /// Fails with [`HashError::NotCryptographic`] for unknown and
    /// non-cryptographic algorithms.
    pub fn new(algo: &str, key: &[u8]) -> Result<Self, HashError> {
        Self::with_options(crypto_algorithm(algo)?, &HashOptions::none(), key)
    }

    fn with_options(
        algo: &'static HashAlgorithm,
        options: &HashOptions,
        key: &[u8],
    ) -> Result<Self, HashError> {
        let (ipad, opad) = pads(algo, options, key);
        let mut inner = algo.new_hasher(options)?;
        inner.update(&ipad);
        let mut outer = algo.new_hasher(options)?;
        outer.update(&opad);
        Ok(Self { algo, inner, outer })
    }

    /// The registry entry of the underlying hash.
    pub fn algorithm(&self) -> &'static HashAlgorithm {
        self.algo
    }

    /// Feeds more message data.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Consumes the context and returns the raw MAC.
    pub fn finalize(self) -> Vec<u8> {
        let digest = self.inner.finalize();
        let mut outer = self.outer;
        outer.update(&digest);
        outer.finalize()
    }
}

/// One-shot HMAC (`hash_hmac($algo, $data, $key, true)`).
pub fn hmac(algo: &str, key: &[u8], data: &[u8]) -> Result<Vec<u8>, HashError> {
    let mut mac = Hmac::new(algo, key)?;
    mac.update(data);
    Ok(mac.finalize())
}

/// A `hash_init()` context: a plain hash, or an HMAC when a key was supplied.
#[derive(Clone)]
pub struct HashContext {
    algo: &'static HashAlgorithm,
    kind: ContextKind,
}

#[derive(Clone)]
enum ContextKind {
    Plain(Box<dyn Hasher>),
    Hmac(Hmac),
}

impl HashContext {
    /// `hash_init($algo, 0, '', $options)`.
    pub fn new(algo: &str, options: &HashOptions) -> Result<Self, HashError> {
        let algo = find(algo).ok_or(HashError::UnknownAlgorithm)?;
        Ok(Self {
            algo,
            kind: ContextKind::Plain(algo.new_hasher(options)?),
        })
    }

    /// `hash_init($algo, HASH_HMAC, $key, $options)`.
    ///
    /// Errors: [`HashError::UnknownAlgorithm`], [`HashError::NotCryptographic`]
    /// and [`HashError::EmptyKey`] (a zero-length key is no key at all here).
    pub fn new_hmac(algo: &str, key: &[u8], options: &HashOptions) -> Result<Self, HashError> {
        let algo = find(algo).ok_or(HashError::UnknownAlgorithm)?;
        if !algo.is_crypto {
            return Err(HashError::NotCryptographic);
        }
        if key.is_empty() {
            return Err(HashError::EmptyKey);
        }
        Ok(Self {
            algo,
            kind: ContextKind::Hmac(Hmac::with_options(algo, options, key)?),
        })
    }

    /// Registry entry of the algorithm.
    pub fn algorithm(&self) -> &'static HashAlgorithm {
        self.algo
    }

    /// Whether the context was created with `HASH_HMAC`.
    pub fn is_hmac(&self) -> bool {
        matches!(self.kind, ContextKind::Hmac(_))
    }

    /// `hash_update()`.
    pub fn update(&mut self, data: &[u8]) {
        match &mut self.kind {
            ContextKind::Plain(h) => h.update(data),
            ContextKind::Hmac(m) => m.update(data),
        }
    }

    /// `hash_final()` (raw bytes).
    pub fn finalize(self) -> Vec<u8> {
        match self.kind {
            ContextKind::Plain(h) => h.finalize(),
            ContextKind::Hmac(m) => m.finalize(),
        }
    }

    /// `HashContext::__serialize()` state; fails for HMAC contexts (PHP refuses
    /// to serialise them, the key would be exposed) and for `xxh3`/`xxh128`.
    pub fn serialize_state(&self) -> Result<(i64, Vec<StateItem>), StateError> {
        match &self.kind {
            ContextKind::Plain(h) => h.serialize_state(),
            ContextKind::Hmac(_) => Err(StateError::Failure),
        }
    }

    /// `HashContext::__unserialize()` for the state produced by
    /// [`HashContext::serialize_state`]: creates a fresh context for `algo`
    /// and loads the state into it.
    pub fn from_state(algo: &str, magic: i64, state: &[StateItem]) -> Result<Self, HashError> {
        let mut ctx = Self::new(algo, &HashOptions::none())?;
        let ContextKind::Plain(h) = &mut ctx.kind else {
            unreachable!("new() builds plain contexts");
        };
        h.unserialize_state(magic, state)
            .map_err(HashError::InvalidState)?;
        Ok(ctx)
    }
}

/// `hash_pbkdf2()`: PBKDF2-HMAC (RFC 8018).
///
/// `length` follows PHP: when `raw` is false it counts hexadecimal characters
/// (an odd count truncates the last digit); `0` selects the full digest size
/// (twice that in hex).  The result is raw bytes for `raw == true` and
/// lower-case ASCII hex otherwise.  `iterations` must be at least 1.
pub fn pbkdf2(
    algo: &str,
    password: &[u8],
    salt: &[u8],
    iterations: u64,
    length: usize,
    raw: bool,
) -> Result<Vec<u8>, HashError> {
    let algo = crypto_algorithm(algo)?;
    if iterations == 0 {
        return Err(HashError::InvalidIterations);
    }
    let digest_size = algo.digest_size;
    let mut length = length;
    if length == 0 {
        length = if raw { digest_size } else { digest_size * 2 };
    }
    let digest_length = if raw { length } else { length / 2 + length % 2 };
    let loops = (digest_length - 1) / digest_size + 1;
    // PHP would run out of memory here; report it as an error instead.
    let capacity = loops
        .checked_mul(digest_size)
        .filter(|&c| c <= isize::MAX as usize / 2)
        .ok_or(HashError::LengthTooLarge {
            max: isize::MAX as usize / 2,
        })?;

    let base = Hmac::with_options(algo, &HashOptions::none(), password)?;
    let mut result = Vec::with_capacity(capacity);
    let mut salted = salt.to_vec();
    salted.extend_from_slice(&[0; 4]);
    for block_index in 1..=loops {
        let n = salted.len();
        salted[n - 4..].copy_from_slice(&(block_index as u32).to_be_bytes());
        let mut mac = base.clone();
        mac.update(&salted);
        let mut digest = mac.finalize();
        let mut temp = digest.clone();
        for _ in 1..iterations {
            let mut mac = base.clone();
            mac.update(&digest);
            digest = mac.finalize();
            for (t, d) in temp.iter_mut().zip(&digest) {
                *t ^= d;
            }
        }
        result.extend_from_slice(&temp);
    }
    if raw {
        result.truncate(length);
        Ok(result)
    } else {
        let mut hex = crate::hasher::to_hex(&result[..digest_length]).into_bytes();
        hex.truncate(length);
        Ok(hex)
    }
}

/// `hash_hkdf()`: HKDF (RFC 5869) returning raw bytes.
///
/// `length == 0` selects the digest size; the maximum is `255 * digest_size`
/// ([`HashError::LengthTooLarge`]).  An empty `ikm` is rejected
/// ([`HashError::EmptyInput`]); an empty `salt` or `info` is fine.
pub fn hkdf(
    algo: &str,
    ikm: &[u8],
    length: usize,
    info: &[u8],
    salt: &[u8],
) -> Result<Vec<u8>, HashError> {
    let algo = crypto_algorithm(algo)?;
    if ikm.is_empty() {
        return Err(HashError::EmptyInput);
    }
    let digest_size = algo.digest_size;
    let max = digest_size * 255;
    let length = match length {
        0 => digest_size,
        l if l > max => return Err(HashError::LengthTooLarge { max }),
        l => l,
    };

    // Extract
    let mut extract = Hmac::with_options(algo, &HashOptions::none(), salt)?;
    extract.update(ikm);
    let prk = extract.finalize();

    // Expand
    let expander = Hmac::with_options(algo, &HashOptions::none(), &prk)?;
    let rounds = (length - 1) / digest_size + 1;
    let mut out = Vec::with_capacity(rounds * digest_size);
    let mut previous: Vec<u8> = Vec::new();
    for i in 1..=rounds {
        let mut mac = expander.clone();
        mac.update(&previous);
        mac.update(info);
        mac.update(&[(i & 0xff) as u8]);
        previous = mac.finalize();
        out.extend_from_slice(&previous);
    }
    out.truncate(length);
    Ok(out)
}
