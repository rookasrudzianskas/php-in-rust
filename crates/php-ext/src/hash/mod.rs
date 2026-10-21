//! ext/hash — the `hash*()` functions of `ext/hash/hash.c`, built on the
//! algorithm library in the `php-hash` crate.
//!
//! * this file: the functional API (`hash`, `hash_file`, `hash_hmac`,
//!   `hash_hmac_file`, `hash_pbkdf2`, `hash_hkdf`, `hash_equals`, `hash_algos`,
//!   `hash_hmac_algos`) and the helpers shared by the submodules;
//! * `context`: the final class `HashContext` and the incremental API
//!   (`hash_init`, `hash_update*`, `hash_final`, `hash_copy`);
//! * `mhash`: the deprecated `mhash*()` compatibility layer and its constants.

mod context;
mod mhash;

use php_hash::{HashAlgorithm, HashError, HashOptions, Hmac, to_hex};
use php_runtime::{Args, Flow, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use crate::standard::streams::read_all;

/// `HASH_HMAC` (`PHP_HASH_HMAC`): the `$flags` bit selecting HMAC mode.
pub(crate) const HASH_HMAC: i64 = 0x0001;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// `php_hash_fetch_ops()`: algorithm lookup, ASCII case-insensitive.
fn fetch_algo(name: &[u8]) -> Option<&'static HashAlgorithm> {
    std::str::from_utf8(name).ok().and_then(php_hash::find)
}

/// Resolves the `$algo` argument (#1) or throws
/// `ValueError: ... must be a valid hashing algorithm`.
fn algo_arg(vm: &mut Vm, name: &[u8]) -> PResult<&'static HashAlgorithm> {
    match fetch_algo(name) {
        Some(algo) => Ok(algo),
        None => Err(vm.argument_value_error(1, "must be a valid hashing algorithm")),
    }
}

/// Like [`algo_arg`] for the HMAC family: only cryptographic algorithms qualify.
fn crypto_algo_arg(vm: &mut Vm, name: &[u8]) -> PResult<&'static HashAlgorithm> {
    match fetch_algo(name) {
        Some(algo) if algo.is_crypto => Ok(algo),
        _ => Err(vm.argument_value_error(1, "must be a valid cryptographic hashing algorithm")),
    }
}

/// `Z_PARAM_PATH` / `P`: paths must not contain NUL bytes.
pub(crate) fn check_path(vm: &mut Vm, path: &[u8], arg_num: usize) -> PResult<()> {
    if path.contains(&0) {
        Err(vm.argument_value_error(arg_num, "must not contain any null bytes"))
    } else {
        Ok(())
    }
}

/// The digest as PHP returns it: raw bytes or lower-case hexadecimal.
pub(crate) fn digest_value(digest: Vec<u8>, raw: bool) -> Value {
    if raw {
        Value::String(PhpStr::from_vec(digest))
    } else {
        Value::String(PhpStr::from(to_hex(&digest)))
    }
}

/// Maps an error raised by an algorithm's `hash_init` (`zend_throw_error`) to
/// the thrown `Error`.
fn init_error(vm: &mut Vm, err: HashError) -> Flow {
    vm.throw_error("Error", err.to_string())
}

/// Emulates PHP's out-of-memory fatal error for allocations that could never
/// succeed, so a huge `$length` does not abort the whole process.
pub(crate) fn check_allocation(vm: &mut Vm, bytes: u128) -> PResult<()> {
    let configured = vm.ini_int("memory_limit");
    // `-1` (or 0) means unlimited; Rust's allocator limit is the next best thing.
    let limit = if configured > 0 {
        configured as u128
    } else {
        isize::MAX as u128 / 2
    };
    if bytes > limit {
        return Err(vm.fatal_error(format!(
            "Allowed memory size of {limit} bytes exhausted (tried to allocate {bytes} bytes)"
        )));
    }
    Ok(())
}

/// Builds the algorithm options from the `$options` array, running the
/// diagnostics of the algorithms' `hash_init` functions
/// (`ext/hash/hash_murmur.c`, `ext/hash/hash_xxhash.c`).
///
/// Only the murmur3 and xxh families look at the array; every other algorithm
/// ignores it.
fn init_options(
    vm: &mut Vm,
    algo: &HashAlgorithm,
    options: Option<&PhpArray>,
) -> PResult<HashOptions> {
    let Some(options) = options else {
        return Ok(HashOptions::none());
    };
    let lookup = |key: &[u8]| options.get_str(key).map(Value::unref);
    match algo.name {
        "murmur3a" | "murmur3c" | "murmur3f" | "xxh32" | "xxh64" => match lookup(b"seed") {
            Some(Value::Int(seed)) => Ok(HashOptions::with_seed(seed)),
            Some(_) => {
                vm.deprecated(
                    "Passing a seed of a type other than int is deprecated because it is the \
                     same as setting the seed to 0",
                )?;
                Ok(HashOptions::none())
            }
            None => Ok(HashOptions::none()),
        },
        "xxh3" | "xxh128" => {
            let (seed, secret) = (lookup(b"seed"), lookup(b"secret"));
            if seed.is_some() && secret.is_some() {
                return Err(init_error(
                    vm,
                    HashError::SeedAndSecret {
                        algo: algo_label(algo),
                    },
                ));
            }
            match (seed, secret) {
                (Some(Value::Int(seed)), _) => Ok(HashOptions::with_seed(seed)),
                (Some(_), _) => {
                    vm.deprecated(
                        "Passing a seed of a type other than int is deprecated because it is \
                         ignored",
                    )?;
                    Ok(HashOptions::none())
                }
                (None, Some(secret)) => {
                    if !matches!(secret, Value::String(_)) {
                        vm.deprecated(
                            "Passing a secret of a type other than string is deprecated because \
                             it implicitly converts to a string, potentially hiding bugs",
                        )?;
                    }
                    let secret = vm.to_php_string(&secret)?;
                    if secret.len() < php_hash::XXH3_SECRET_SIZE_MIN {
                        return Err(init_error(
                            vm,
                            HashError::SecretTooShort {
                                algo: algo_label(algo),
                                len: secret.len(),
                            },
                        ));
                    }
                    if secret.len() > php_hash::XXH3_SECRET_SIZE_MAX {
                        vm.warning(format!(
                            "{}: Secret content exceeding {} bytes discarded",
                            algo.name,
                            php_hash::XXH3_SECRET_SIZE_MAX
                        ))?;
                    }
                    Ok(HashOptions::with_secret(secret.as_bytes()))
                }
                (None, None) => Ok(HashOptions::none()),
            }
        }
        _ => Ok(HashOptions::none()),
    }
}

/// The `&'static str` algorithm name `HashError` wants for xxh3 / xxh128.
fn algo_label(algo: &HashAlgorithm) -> &'static str {
    if algo.name == "xxh128" {
        "xxh128"
    } else {
        "xxh3"
    }
}

// ---------------------------------------------------------------------------
// hash(), hash_file()
// ---------------------------------------------------------------------------

/// `php_hash_do_hash()`.
fn do_hash(vm: &mut Vm, args: &Args, is_file: bool) -> PResult<Value> {
    let algo = algo_arg(vm, args.str(0))?;
    let raw = args.bool(2);
    let file_data;
    let data: &[u8] = if is_file {
        check_path(vm, args.str(1), 2)?;
        match read_all(vm, args.str(1))? {
            Some(contents) => {
                file_data = contents;
                &file_data
            }
            // The stream layer has already reported the problem.
            None => return Ok(Value::Bool(false)),
        }
    } else {
        args.str(1)
    };
    let options = init_options(vm, algo, Some(&**args.array(3)))?;
    let mut hasher = algo.new_hasher(&options).map_err(|e| init_error(vm, e))?;
    hasher.update(data);
    Ok(digest_value(hasher.finalize(), raw))
}

fn php_hash(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_hash(vm, args, false)
}

fn php_hash_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_hash(vm, args, true)
}

// ---------------------------------------------------------------------------
// hash_hmac(), hash_hmac_file()
// ---------------------------------------------------------------------------

/// The HMAC of `data` under `key` (`php_hash_do_hash_hmac()` after the
/// argument checks); `algo` must be cryptographic.
pub(crate) fn hmac_digest(algo: &HashAlgorithm, data: &[u8], key: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::new(algo.name, key).expect("cryptographic algorithm");
    mac.update(data);
    mac.finalize()
}

/// `php_hash_do_hash_hmac()`.
fn do_hash_hmac(vm: &mut Vm, args: &Args, is_file: bool) -> PResult<Value> {
    let algo = crypto_algo_arg(vm, args.str(0))?;
    let raw = args.bool(3);
    let file_data;
    let data: &[u8] = if is_file {
        check_path(vm, args.str(1), 2)?;
        match read_all(vm, args.str(1))? {
            Some(contents) => {
                file_data = contents;
                &file_data
            }
            None => return Ok(Value::Bool(false)),
        }
    } else {
        args.str(1)
    };
    Ok(digest_value(hmac_digest(algo, data, args.str(2)), raw))
}

fn php_hash_hmac(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_hash_hmac(vm, args, false)
}

fn php_hash_hmac_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_hash_hmac(vm, args, true)
}

// ---------------------------------------------------------------------------
// hash_algos(), hash_hmac_algos()
// ---------------------------------------------------------------------------

fn name_list(names: Vec<&'static str>) -> Value {
    Value::array(PhpArray::from_values(
        names.into_iter().map(Value::str).collect(),
    ))
}

fn php_hash_algos(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(name_list(php_hash::hash_algos()))
}

fn php_hash_hmac_algos(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(name_list(php_hash::hash_hmac_algos()))
}

// ---------------------------------------------------------------------------
// hash_pbkdf2(), hash_hkdf()
// ---------------------------------------------------------------------------

fn php_hash_pbkdf2(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let algo = crypto_algo_arg(vm, args.str(0))?;
    let (password, salt) = (args.str(1), args.str(2));
    let (iterations, length, raw) = (args.int(3), args.int(4), args.bool(5));

    if salt.len() > i32::MAX as usize - 4 {
        return Err(vm.argument_value_error(3, "must be less than or equal to INT_MAX - 4 bytes"));
    }
    if iterations <= 0 {
        return Err(vm.argument_value_error(4, "must be greater than 0"));
    }
    if length < 0 {
        return Err(vm.argument_value_error(5, "must be greater than or equal to 0"));
    }

    // PHP allocates the whole key stream up front; refuse what it could not.
    let digest_size = algo.digest_size as u128;
    let mut wanted = length as u128;
    if wanted == 0 {
        wanted = if raw { digest_size } else { digest_size * 2 };
    }
    let digest_length = if raw { wanted } else { wanted / 2 + wanted % 2 };
    let loops = (digest_length - 1) / digest_size + 1;
    check_allocation(vm, loops * digest_size)?;

    match php_hash::pbkdf2(
        algo.name,
        password,
        salt,
        iterations as u64,
        length as usize,
        raw,
    ) {
        Ok(key) => Ok(Value::String(PhpStr::from_vec(key))),
        Err(e) => Err(vm.throw_value_error(e.to_string())),
    }
}

fn php_hash_hkdf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let algo = crypto_algo_arg(vm, args.str(0))?;
    let (ikm, length) = (args.str(1), args.int(2));
    let (info, salt) = (args.str(3), args.str(4));

    if ikm.is_empty() {
        return Err(vm.argument_value_error(2, "must not be empty"));
    }
    let max = algo.digest_size * 255;
    if length < 0 {
        return Err(vm.argument_value_error(3, "must be greater than or equal to 0"));
    } else if length > max as i64 {
        return Err(vm.argument_value_error(3, &format!("must be less than or equal to {max}")));
    }
    match php_hash::hkdf(algo.name, ikm, length as usize, info, salt) {
        Ok(okm) => Ok(Value::String(PhpStr::from_vec(okm))),
        Err(e) => Err(vm.throw_value_error(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// hash_equals()
// ---------------------------------------------------------------------------

/// `php_safe_bcmp()`: compares in time independent of where the strings
/// differ (a length difference leaks, as in PHP).
fn safe_equals(known: &[u8], user: &[u8]) -> bool {
    if known.len() != user.len() {
        return false;
    }
    let diff = known
        .iter()
        .zip(user)
        .fold(0u8, |acc, (a, b)| acc | std::hint::black_box(a ^ b));
    diff == 0
}

/// `hash_equals()` declares `string` parameters but parses them with `"zz"`
/// and rejects every other type, even in weak mode, so the stub's types are
/// widened to `mixed` here and checked by hand.
fn php_hash_equals(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    for i in 0..2 {
        if !matches!(args.get(i), Value::String(_)) {
            let msg = format!("must be of type string, {} given", args.get(i).value_name());
            return Err(vm.argument_type_error(i + 1, &msg));
        }
    }
    Ok(Value::Bool(safe_equals(args.str(0), args.str(1))))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

static HASH: NativeFunction = NativeFunction {
    sig: "hash(string $algo, string $data, bool $binary = false, array $options = []): string",
    handler: php_hash,
};
static HASH_FILE: NativeFunction = NativeFunction {
    sig: "hash_file(string $algo, string $filename, bool $binary = false, array $options = []): string|false",
    handler: php_hash_file,
};
static HASH_HMAC_FN: NativeFunction = NativeFunction {
    sig: "hash_hmac(string $algo, string $data, #[\\SensitiveParameter] string $key, bool $binary = false): string",
    handler: php_hash_hmac,
};
static HASH_HMAC_FILE: NativeFunction = NativeFunction {
    sig: "hash_hmac_file(string $algo, string $filename, #[\\SensitiveParameter] string $key, bool $binary = false): string|false",
    handler: php_hash_hmac_file,
};
static HASH_ALGOS: NativeFunction = NativeFunction {
    sig: "hash_algos(): array",
    handler: php_hash_algos,
};
static HASH_HMAC_ALGOS: NativeFunction = NativeFunction {
    sig: "hash_hmac_algos(): array",
    handler: php_hash_hmac_algos,
};
static HASH_PBKDF2: NativeFunction = NativeFunction {
    sig: "hash_pbkdf2(string $algo, #[\\SensitiveParameter] string $password, string $salt, int $iterations, int $length = 0, bool $binary = false, array $options = []): string",
    handler: php_hash_pbkdf2,
};
static HASH_EQUALS: NativeFunction = NativeFunction {
    sig: "hash_equals(#[\\SensitiveParameter] mixed $known_string, #[\\SensitiveParameter] mixed $user_string): bool",
    handler: php_hash_equals,
};
static HASH_HKDF: NativeFunction = NativeFunction {
    sig: "hash_hkdf(string $algo, #[\\SensitiveParameter] string $key, int $length = 0, string $info = \"\", string $salt = \"\"): string",
    handler: php_hash_hkdf,
};

pub fn register(r: &mut Registry) {
    r.constant("HASH_HMAC", HASH_HMAC);

    r.function(&HASH);
    r.function(&HASH_FILE);
    r.function(&HASH_HMAC_FN);
    r.function(&HASH_HMAC_FILE);
    context::register_functions(r);
    r.function(&HASH_ALGOS);
    r.function(&HASH_HMAC_ALGOS);
    r.function(&HASH_PBKDF2);
    r.function(&HASH_EQUALS);
    r.function(&HASH_HKDF);
    mhash::register(r);
    context::register_class(r);
}
