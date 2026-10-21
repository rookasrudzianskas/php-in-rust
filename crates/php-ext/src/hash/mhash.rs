//! The deprecated `mhash*()` compatibility layer (`PHP_MHASH_BC` in
//! `ext/hash/hash.c`): the `MHASH_*` constants and `mhash()`,
//! `mhash_get_hash_name()`, `mhash_get_block_size()`, `mhash_keygen_s2k()` and
//! `mhash_count()`.

use php_hash::{HashAlgorithm, HashOptions};
use php_runtime::class::fn_flags;
use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

use super::{check_allocation, digest_value, hmac_digest};

/// `mhash_to_hash`: `(mhash name, hash() algorithm)` by `MHASH_*` value.
/// Gaps are algorithms mhash knew but PHP never supported.
const MHASH_TO_HASH: [Option<(&str, &str)>; MHASH_NUM_ALGOS] = [
    Some(("CRC32", "crc32")),
    Some(("MD5", "md5")),
    Some(("SHA1", "sha1")),
    Some(("HAVAL256", "haval256,3")),
    None,
    Some(("RIPEMD160", "ripemd160")),
    None,
    Some(("TIGER", "tiger192,3")),
    Some(("GOST", "gost")),
    Some(("CRC32B", "crc32b")),
    Some(("HAVAL224", "haval224,3")),
    Some(("HAVAL192", "haval192,3")),
    Some(("HAVAL160", "haval160,3")),
    Some(("HAVAL128", "haval128,3")),
    Some(("TIGER128", "tiger128,3")),
    Some(("TIGER160", "tiger160,3")),
    Some(("MD4", "md4")),
    Some(("SHA256", "sha256")),
    Some(("ADLER32", "adler32")),
    Some(("SHA224", "sha224")),
    Some(("SHA512", "sha512")),
    Some(("SHA384", "sha384")),
    Some(("WHIRLPOOL", "whirlpool")),
    Some(("RIPEMD128", "ripemd128")),
    Some(("RIPEMD256", "ripemd256")),
    Some(("RIPEMD320", "ripemd320")),
    None,
    Some(("SNEFRU256", "snefru256")),
    Some(("MD2", "md2")),
    Some(("FNV132", "fnv132")),
    Some(("FNV1A32", "fnv1a32")),
    Some(("FNV164", "fnv164")),
    Some(("FNV1A64", "fnv1a64")),
    Some(("JOAAT", "joaat")),
    Some(("CRC32C", "crc32c")),
    Some(("MURMUR3A", "murmur3a")),
    Some(("MURMUR3C", "murmur3c")),
    Some(("MURMUR3F", "murmur3f")),
    Some(("XXH32", "xxh32")),
    Some(("XXH64", "xxh64")),
    Some(("XXH3", "xxh3")),
    Some(("XXH128", "xxh128")),
];

/// `MHASH_NUM_ALGOS`.
const MHASH_NUM_ALGOS: usize = 42;

/// `SALT_SIZE` of `mhash_keygen_s2k()`.
const SALT_SIZE: usize = 8;

/// The `(mhash name, algorithm)` an `MHASH_*` value stands for.
fn mhash_entry(algorithm: i64) -> Option<(&'static str, &'static HashAlgorithm)> {
    let index = usize::try_from(algorithm).ok()?;
    let (mhash_name, hash_name) = (*MHASH_TO_HASH.get(index)?)?;
    Some((mhash_name, php_hash::find(hash_name)?))
}

fn mhash(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some((_, algo)) = mhash_entry(args.int(0)) else {
        return Ok(Value::Bool(false));
    };
    let data = args.str(1);
    match args.str_opt(2) {
        Some(key) => {
            if !algo.is_crypto {
                return Err(
                    vm.argument_value_error(1, "must be a valid cryptographic hashing algorithm")
                );
            }
            Ok(digest_value(hmac_digest(algo, data, key), true))
        }
        None => {
            let mut hasher = algo
                .new_hasher(&HashOptions::none())
                .expect("default options are always valid");
            hasher.update(data);
            Ok(digest_value(hasher.finalize(), true))
        }
    }
}

fn mhash_get_hash_name(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match mhash_entry(args.int(0)) {
        Some((mhash_name, _)) => Value::str(mhash_name),
        None => Value::Bool(false),
    })
}

fn mhash_count(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(MHASH_NUM_ALGOS as i64 - 1))
}

/// Despite its name this is the digest size.
fn mhash_get_block_size(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match mhash_entry(args.int(0)) {
        Some((_, algo)) => Value::Int(algo.digest_size as i64),
        None => Value::Bool(false),
    })
}

/// The "simple string-to-key" generator of RFC 2440 (without iteration).
fn mhash_keygen_s2k(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (algorithm, password, salt, bytes) = (args.int(0), args.str(1), args.str(2), args.int(3));

    if bytes <= 0 {
        return Err(vm.argument_value_error(4, "must be a greater than 0"));
    }
    if bytes > i64::from(i32::MAX) {
        return Err(
            vm.argument_value_error(4, &format!("must be less than or equal to {}", i32::MAX))
        );
    }
    let bytes = bytes as usize;

    // The salt is cut or zero padded to exactly SALT_SIZE bytes.
    let mut padded_salt = [0u8; SALT_SIZE];
    let salt_len = salt.len().min(SALT_SIZE);
    padded_salt[..salt_len].copy_from_slice(&salt[..salt_len]);

    let Some((_, algo)) = mhash_entry(algorithm) else {
        return Ok(Value::Bool(false));
    };
    let block_size = algo.digest_size;
    let times = bytes.div_ceil(block_size);
    check_allocation(vm, (times * block_size) as u128)?;

    const ZEROS: [u8; 64] = [0; 64];
    let mut key = Vec::with_capacity(times * block_size);
    for i in 0..times {
        let mut hasher = algo
            .new_hasher(&HashOptions::none())
            .expect("default options are always valid");
        // Round `i` is prefixed with `i` NUL bytes.
        let mut remaining = i;
        while remaining > 0 {
            let n = remaining.min(ZEROS.len());
            hasher.update(&ZEROS[..n]);
            remaining -= n;
        }
        hasher.update(&padded_salt);
        hasher.update(password);
        key.extend_from_slice(&hasher.finalize());
    }
    key.truncate(bytes);
    Ok(Value::String(PhpStr::from_vec(key)))
}

static MHASH_GET_BLOCK_SIZE: NativeFunction = NativeFunction {
    sig: "mhash_get_block_size(int $algo): int|false",
    handler: mhash_get_block_size,
};
static MHASH_GET_HASH_NAME: NativeFunction = NativeFunction {
    sig: "mhash_get_hash_name(int $algo): string|false",
    handler: mhash_get_hash_name,
};
static MHASH_KEYGEN_S2K: NativeFunction = NativeFunction {
    sig: "mhash_keygen_s2k(int $algo, #[\\SensitiveParameter] string $password, string $salt, int $length): string|false",
    handler: mhash_keygen_s2k,
};
static MHASH_COUNT: NativeFunction = NativeFunction {
    sig: "mhash_count(): int",
    handler: mhash_count,
};
static MHASH: NativeFunction = NativeFunction {
    sig: "mhash(int $algo, string $data, #[\\SensitiveParameter] ?string $key = null): string|false",
    handler: mhash,
};

/// Registers the deprecated functions (`#[\Deprecated(since: '8.1')]`) and the
/// deprecated `MHASH_*` constants (`mhash_init()`).
pub(super) fn register(r: &mut Registry) {
    for nf in [
        &MHASH_GET_BLOCK_SIZE,
        &MHASH_GET_HASH_NAME,
        &MHASH_KEYGEN_S2K,
        &MHASH_COUNT,
        &MHASH,
    ] {
        r.function_with_flags(nf, fn_flags::DEPRECATED);
    }
    for (value, entry) in MHASH_TO_HASH.iter().enumerate() {
        if let Some((mhash_name, _)) = entry {
            r.deprecated_constant(
                &format!("MHASH_{mhash_name}"),
                value as i64,
                "since 8.5, as the mhash*() functions were deprecated",
            );
        }
    }
}
