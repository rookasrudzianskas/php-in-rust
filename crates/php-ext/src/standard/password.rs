//! ext/standard/password.c — `password_hash()` and friends.

use php_runtime::{Args, Flow, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::crypt::{php_crypt, php_safe_bcmp};

mod argon2;
mod blake2b;

/// `PHP_PASSWORD_BCRYPT_COST`.
pub const PASSWORD_BCRYPT_COST: i64 = 12;
/// `PHP_PASSWORD_ARGON2_MEMORY_COST` (`64 << 10` KiB).
pub const PASSWORD_ARGON2_MEMORY_COST: i64 = 64 << 10;
/// `PHP_PASSWORD_ARGON2_TIME_COST`.
pub const PASSWORD_ARGON2_TIME_COST: i64 = 4;
/// `PHP_PASSWORD_ARGON2_THREADS`.
pub const PASSWORD_ARGON2_THREADS: i64 = 1;

/// The registered password hashing algorithms (`php_password_algos`), in
/// registration order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Algo {
    Bcrypt,
    Argon2i,
    Argon2id,
}

/// Registered algorithms in `php_password_algo_register()` order.
const ALGOS: &[Algo] = &[Algo::Bcrypt, Algo::Argon2i, Algo::Argon2id];

impl Algo {
    /// The registry key (`ident`), as in `$2y$...`.
    fn ident(self) -> &'static str {
        match self {
            Algo::Bcrypt => "2y",
            Algo::Argon2i => "argon2i",
            Algo::Argon2id => "argon2id",
        }
    }

    /// `php_password_algo.name`.
    fn name(self) -> &'static str {
        match self {
            Algo::Bcrypt => "bcrypt",
            Algo::Argon2i => "argon2i",
            Algo::Argon2id => "argon2id",
        }
    }

    /// `php_password_algo_find()`.
    fn find(ident: &[u8]) -> Option<Algo> {
        ALGOS
            .iter()
            .copied()
            .find(|a| a.ident().as_bytes() == ident)
    }

    /// `php_password_algo.valid`: algorithms without a `valid` callback
    /// accept every hash that carries their identifier.
    fn valid(self, hash: &[u8]) -> bool {
        match self {
            Algo::Bcrypt => bcrypt_get_cost(hash).is_some(),
            Algo::Argon2i | Algo::Argon2id => true,
        }
    }

    /// `php_password_algo.get_info`.
    fn get_info(self, options: &mut PhpArray, hash: &[u8]) {
        match self {
            Algo::Bcrypt => {
                if let Some(cost) = bcrypt_get_cost(hash) {
                    options.insert_str("cost", cost);
                }
            }
            Algo::Argon2i | Algo::Argon2id => argon2_get_info(options, hash),
        }
    }

    /// `php_password_algo.needs_rehash`.
    fn needs_rehash(self, vm: &mut Vm, hash: &[u8], options: &PhpArray) -> PResult<bool> {
        match self {
            Algo::Bcrypt => bcrypt_needs_rehash(vm, hash, options),
            Algo::Argon2i | Algo::Argon2id => argon2_needs_rehash(vm, hash, options),
        }
    }

    /// `php_password_algo.verify`.
    fn verify(self, password: &[u8], hash: &[u8]) -> bool {
        match self {
            Algo::Bcrypt => bcrypt_verify(password, hash),
            Algo::Argon2i => argon2_verify(password, hash, argon2::Type::I),
            Algo::Argon2id => argon2_verify(password, hash, argon2::Type::Id),
        }
    }

    /// `php_password_algo.hash`: `Ok(None)` is php-src's NULL return without a
    /// pending exception.
    fn hash(self, vm: &mut Vm, password: &[u8], options: &PhpArray) -> PResult<Option<Vec<u8>>> {
        match self {
            Algo::Bcrypt => bcrypt_hash(vm, password, options),
            Algo::Argon2i => argon2_hash(vm, password, options, argon2::Type::I),
            Algo::Argon2id => argon2_hash(vm, password, options, argon2::Type::Id),
        }
    }
}

/// `php_password_algo_default()`.
fn algo_default() -> Algo {
    Algo::Bcrypt
}

/// `php_password_algo_extract_ident()`: the text between the first character
/// and the next `$` (the hash is a C string, so a NUL ends the search).
fn extract_ident(hash: &[u8]) -> Option<&[u8]> {
    if hash.len() < 3 {
        // Minimum prefix: "$x$"
        return None;
    }
    let rest = &hash[1..];
    let end = rest.iter().position(|&c| c == b'$' || c == 0)?;
    if rest[end] == 0 {
        // No terminating '$'
        return None;
    }
    Some(&rest[..end])
}

/// `php_password_algo_identify_ex()`.
fn identify_ex(hash: &[u8], default_algo: Option<Algo>) -> Option<Algo> {
    let Some(ident) = extract_ident(hash) else {
        return default_algo;
    };
    match Algo::find(ident) {
        Some(algo) if algo.valid(hash) => Some(algo),
        _ => default_algo,
    }
}

/// `php_password_algo_find_zval()` for an `string|int|null` argument.
fn algo_find_zval(arg: &Value) -> Option<Algo> {
    match arg {
        Value::Null | Value::Undef => Some(algo_default()),
        Value::String(s) => Algo::find(s.as_bytes()),
        Value::Int(n) => algo_find_long(*n),
        Value::Bool(b) => algo_find_long(*b as i64),
        _ => None,
    }
}

fn algo_find_long(n: i64) -> Option<Algo> {
    match n {
        0 => Some(algo_default()),
        1 => Some(Algo::Bcrypt),
        2 => Some(Algo::Argon2i),
        3 => Some(Algo::Argon2id),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Salt generation
// ---------------------------------------------------------------------------

/// Strips Rust's " (os error N)" suffix to get `strerror()` text.
fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// `php_random_bytes_ex()`; the error is the `errstr` php-src would throw as
/// `Random\RandomException`.
#[cfg(unix)]
fn random_bytes(size: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|e| format!("Cannot open /dev/urandom: {}", strerror(&e)))?;
    let mut buf = vec![0u8; size];
    file.read_exact(&mut buf)
        .map_err(|e| format!("Could not gather sufficient random data: {}", strerror(&e)))?;
    Ok(buf)
}

/// Without `/dev/urandom`, fall back to the operating-system seeded keys of
/// the standard library's hasher.
#[cfg(not(unix))]
fn random_bytes(size: usize) -> Result<Vec<u8>, String> {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let state = RandomState::new();
    let mut out = Vec::with_capacity(size + 8);
    let mut counter = 0u64;
    while out.len() < size {
        let mut h = state.build_hasher();
        h.write_u64(counter);
        counter += 1;
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(size);
    Ok(out)
}

/// `php_base64_encode()` (standard alphabet, `=` padding).
fn base64_encode(src: &[u8]) -> Vec<u8> {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(src.len().div_ceil(3) * 4);
    for chunk in src.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63]);
        out.push(B64[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63]
        } else {
            b'='
        });
    }
    out
}

/// php-src's `php_random_bytes_throw()` failing and `password.c` reporting
/// `ValueError: Unable to generate salt` on top of the RandomException.
fn salt_generation_failed(vm: &mut Vm, errstr: String) -> Flow {
    let previous = vm.create_exception("Random\\RandomException", errstr, 0);
    let exception = vm.create_exception("ValueError", "Unable to generate salt", 0);
    exception.set_prop(b"previous", Value::Object(previous));
    Flow::Exception(exception)
}

/// `php_password_make_salt()`: `length` characters of `[./A-Za-z0-9]`.
fn make_salt_with(
    vm: &mut Vm,
    length: usize,
    random: impl FnOnce(usize) -> Result<Vec<u8>, String>,
) -> PResult<Vec<u8>> {
    if length > (i32::MAX as usize) / 3 {
        return Err(vm.throw_value_error("Length is too large to safely generate"));
    }

    let buffer = match random(length * 3 / 4 + 1) {
        Ok(b) => b,
        Err(errstr) => return Err(salt_generation_failed(vm, errstr)),
    };

    let encoded = base64_encode(&buffer);
    if encoded.len() < length {
        // Too short of an encoded string generated
        return Err(vm.throw_value_error("Generated salt too short"));
    }
    let mut ret = Vec::with_capacity(length);
    for &c in &encoded[..length] {
        match c {
            b'+' => ret.push(b'.'),
            b'=' => return Err(vm.throw_value_error("Generated salt too short")),
            c => ret.push(c),
        }
    }
    Ok(ret)
}

/// `php_password_get_salt()`.
fn get_salt(vm: &mut Vm, required_salt_len: usize, options: &PhpArray) -> PResult<Vec<u8>> {
    if options.get_str(b"salt").is_some() {
        vm.warning(
            "The \"salt\" option has been ignored, since providing a custom salt is no longer supported",
        )?;
    }
    make_salt_with(vm, required_salt_len, random_bytes)
}

// ---------------------------------------------------------------------------
// bcrypt
// ---------------------------------------------------------------------------

/// `php_password_bcrypt_get_cost()`: the cost of a well-formed `$2y$` hash.
fn bcrypt_get_cost(hash: &[u8]) -> Option<i64> {
    if hash.len() != 60
        || hash[0] != b'$'
        || hash[1] != b'2'
        || hash[2] != b'y'
        || hash[3] != b'$'
        || !hash[4].is_ascii_digit()
        || !hash[5].is_ascii_digit()
        || hash[6] != b'$'
    {
        return None;
    }
    let cost = (hash[4] - b'0') as i64 * 10 + (hash[5] - b'0') as i64;
    (4..=31).contains(&cost).then_some(cost)
}

fn bcrypt_needs_rehash(vm: &mut Vm, hash: &[u8], options: &PhpArray) -> PResult<bool> {
    let Some(old_cost) = bcrypt_get_cost(hash) else {
        // Should never get called this way.
        return Ok(true);
    };
    let mut new_cost = PASSWORD_BCRYPT_COST;
    if let Some(v) = options.get_str(b"cost") {
        new_cost = vm.to_int(&v.clone())?;
    }
    Ok(old_cost != new_cost)
}

fn bcrypt_verify(password: &[u8], hash: &[u8]) -> bool {
    let Some(ret) = php_crypt(password, hash) else {
        return false;
    };
    if hash.len() < 13 {
        return false;
    }
    // A constant-time equality check that always checks every byte.
    php_safe_bcmp(&ret, hash) == 0
}

fn bcrypt_hash(vm: &mut Vm, password: &[u8], options: &PhpArray) -> PResult<Option<Vec<u8>>> {
    if password.contains(&0) {
        return Err(vm.throw_value_error("Bcrypt password must not contain null character"));
    }

    let mut cost = PASSWORD_BCRYPT_COST;
    if let Some(v) = options.get_str(b"cost") {
        cost = vm.to_int(&v.clone())?;
    }

    if !(4..=31).contains(&cost) {
        return Err(
            vm.throw_value_error(format!("Invalid bcrypt cost parameter specified: {cost}"))
        );
    }

    let mut hash = format!("$2y${cost:02}$").into_bytes();
    let salt = get_salt(vm, 22, options)?;
    hash.extend_from_slice(&salt);

    // This cast is safe, since both values are defined here in code and cannot overflow.
    let Some(result) = php_crypt(password, &hash) else {
        return Ok(None);
    };
    if result.len() < 13 {
        return Ok(None);
    }
    Ok(Some(result))
}

// ---------------------------------------------------------------------------
// argon2i / argon2id
// ---------------------------------------------------------------------------

/// C `isspace()`.
fn c_isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// A `%ld` conversion of `sscanf()`: skips white space, then an optionally
/// signed decimal that saturates like `strtol()`.
fn scan_long(s: &[u8], pos: &mut usize) -> Option<i64> {
    let mut i = *pos;
    while i < s.len() && c_isspace(s[i]) {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut val: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        let d = (s[i] - b'0') as i64;
        val = if neg {
            val.saturating_mul(10).saturating_sub(d)
        } else {
            val.saturating_mul(10).saturating_add(d)
        };
        i += 1;
    }
    if i == start {
        return None;
    }
    *pos = i;
    Some(val)
}

/// A literal character sequence of an `sscanf()` format.
fn scan_literal(s: &[u8], pos: &mut usize, lit: &[u8]) -> bool {
    if s[*pos..].starts_with(lit) {
        *pos += lit.len();
        true
    } else {
        false
    }
}

/// The values `extract_argon2_parameters()` reads from an encoded hash.
struct Argon2Parameters {
    memory_cost: i64,
    time_cost: i64,
    threads: i64,
}

/// `extract_argon2_parameters()`: starts from `initial`; the fields are only
/// replaced as far as `sscanf("v=%ld$m=%ld,t=%ld,p=%ld")` gets, exactly like
/// the partial assignments of the C code. Hashes that do not even start with
/// an argon2 prefix leave `initial` untouched.
fn extract_argon2_parameters(hash: &[u8], initial: Argon2Parameters) -> Argon2Parameters {
    let mut out = initial;
    if hash.len() < b"$argon2id$".len() + 1 {
        return out;
    }
    // The hash is a C string for sscanf().
    let hash = &hash[..hash.iter().position(|&c| c == 0).unwrap_or(hash.len())];
    let rest = if let Some(r) = hash.strip_prefix(b"$argon2i$") {
        r
    } else if let Some(r) = hash.strip_prefix(b"$argon2id$") {
        r
    } else {
        return out;
    };

    let mut pos = 0;
    if !scan_literal(rest, &mut pos, b"v=") || scan_long(rest, &mut pos).is_none() {
        return out;
    }
    if !scan_literal(rest, &mut pos, b"$m=") {
        return out;
    }
    let Some(m) = scan_long(rest, &mut pos) else {
        return out;
    };
    out.memory_cost = m;
    if !scan_literal(rest, &mut pos, b",t=") {
        return out;
    }
    let Some(t) = scan_long(rest, &mut pos) else {
        return out;
    };
    out.time_cost = t;
    if !scan_literal(rest, &mut pos, b",p=") {
        return out;
    }
    let Some(p) = scan_long(rest, &mut pos) else {
        return out;
    };
    out.threads = p;
    out
}

fn argon2_get_info(options: &mut PhpArray, hash: &[u8]) {
    let p = extract_argon2_parameters(
        hash,
        Argon2Parameters {
            memory_cost: PASSWORD_ARGON2_MEMORY_COST,
            time_cost: PASSWORD_ARGON2_TIME_COST,
            threads: PASSWORD_ARGON2_THREADS,
        },
    );
    options.insert_str("memory_cost", p.memory_cost);
    options.insert_str("time_cost", p.time_cost);
    options.insert_str("threads", p.threads);
}

fn argon2_needs_rehash(vm: &mut Vm, hash: &[u8], options: &PhpArray) -> PResult<bool> {
    let mut new_memory_cost = PASSWORD_ARGON2_MEMORY_COST;
    let mut new_time_cost = PASSWORD_ARGON2_TIME_COST;
    let mut new_threads = PASSWORD_ARGON2_THREADS;

    if let Some(v) = options.get_str(b"memory_cost") {
        new_memory_cost = vm.to_int(&v.clone())?;
    }
    if let Some(v) = options.get_str(b"time_cost") {
        new_time_cost = vm.to_int(&v.clone())?;
    }
    if let Some(v) = options.get_str(b"threads") {
        new_threads = vm.to_int(&v.clone())?;
    }

    let p = extract_argon2_parameters(
        hash,
        Argon2Parameters {
            memory_cost: 0,
            time_cost: 0,
            threads: 0,
        },
    );

    Ok(
        new_time_cost != p.time_cost
            || new_memory_cost != p.memory_cost
            || new_threads != p.threads,
    )
}

fn argon2_hash(
    vm: &mut Vm,
    password: &[u8],
    options: &PhpArray,
    ty: argon2::Type,
) -> PResult<Option<Vec<u8>>> {
    // The costs are `size_t` in php-src: negative values wrap to huge ones.
    let mut memory_cost = PASSWORD_ARGON2_MEMORY_COST;
    if let Some(v) = options.get_str(b"memory_cost") {
        memory_cost = vm.to_int(&v.clone())?;
    }
    if memory_cost as u64 > argon2::MAX_MEMORY || (memory_cost as u64) < argon2::MIN_MEMORY {
        return Err(vm.throw_value_error("Memory cost is outside of allowed memory range"));
    }

    let mut time_cost = PASSWORD_ARGON2_TIME_COST;
    if let Some(v) = options.get_str(b"time_cost") {
        time_cost = vm.to_int(&v.clone())?;
    }
    if time_cost as u64 > argon2::MAX_TIME || (time_cost as u64) < argon2::MIN_TIME {
        return Err(vm.throw_value_error("Time cost is outside of allowed time range"));
    }

    let mut threads = PASSWORD_ARGON2_THREADS;
    if let Some(v) = options.get_str(b"threads") {
        threads = vm.to_int(&v.clone())?;
    }
    if threads as u64 > argon2::MAX_LANES || threads == 0 {
        return Err(vm.throw_value_error("Invalid number of threads"));
    }

    let salt = get_salt(vm, 16, options)?;

    match argon2::hash_encoded(
        ty,
        time_cost as u32,
        memory_cost as u32,
        threads as u32,
        password,
        &salt,
        32,
    ) {
        Ok(encoded) => Ok(Some(encoded)),
        Err(e) => Err(vm.throw_value_error(e.message())),
    }
}

fn argon2_verify(password: &[u8], hash: &[u8], ty: argon2::Type) -> bool {
    // argon2_verify() takes the hash as a C string.
    let hash = &hash[..hash.iter().position(|&c| c == 0).unwrap_or(hash.len())];
    argon2::verify(hash, password, ty).is_ok()
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

/// The `$options` array of `password_hash()` / `password_needs_rehash()`.
fn options_arg(args: &Args, i: usize) -> PhpArray {
    match args.array_opt(i) {
        Some(a) => (**a).clone(),
        None => PhpArray::new(),
    }
}

/// Retrieves information about a given hash
fn password_get_info(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hash = args.str(0).clone();
    let hash = hash.as_bytes();

    let mut ret = PhpArray::new();
    let mut options = PhpArray::new();

    let ident = extract_ident(hash);
    let algo = ident.and_then(Algo::find);
    match algo {
        Some(algo) if algo.valid(hash) => {
            ret.insert_str("algo", Value::from(ident.unwrap_or_default()));
            ret.insert_str("algoName", algo.name());
            algo.get_info(&mut options, hash);
        }
        _ => {
            ret.insert_str("algo", Value::Null);
            ret.insert_str("algoName", "unknown");
        }
    }
    ret.insert_str("options", Value::from(options));
    Ok(Value::from(ret))
}

/// Determines if a given hash requires re-hashing based upon parameters
fn password_needs_rehash(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hash = args.str(0).clone();
    let options = options_arg(args, 2);

    let Some(new_algo) = algo_find_zval(args.get(1)) else {
        // Unknown new algorithm, never prompt to rehash.
        return Ok(Value::Bool(false));
    };

    let old_algo = identify_ex(hash.as_bytes(), None);
    if old_algo != Some(new_algo) {
        // Different algorithm preferred, always rehash.
        return Ok(Value::Bool(true));
    }

    Ok(Value::Bool(new_algo.needs_rehash(
        vm,
        hash.as_bytes(),
        &options,
    )?))
}

/// Verify a hash created using crypt() or password_hash()
fn password_verify(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let password = args.str(0).clone();
    let hash = args.str(1).clone();

    let algo = identify_ex(hash.as_bytes(), Some(algo_default()));
    Ok(Value::Bool(algo.is_some_and(|a| {
        a.verify(password.as_bytes(), hash.as_bytes())
    })))
}

/// Hash a password
fn password_hash(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let password = args.str(0).clone();
    let options = options_arg(args, 2);

    let Some(algo) = algo_find_zval(args.get(1)) else {
        return Err(vm.argument_value_error(2, "must be a valid password hashing algorithm"));
    };

    match algo.hash(vm, password.as_bytes(), &options)? {
        Some(digest) => Ok(Value::from(PhpStr::from_vec(digest))),
        None => Err(vm.throw_error("Error", "Password hashing failed for unknown reason")),
    }
}

fn password_algos(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let mut ret = PhpArray::new();
    for algo in ALGOS {
        let _ = ret.append(algo.ident());
    }
    Ok(Value::from(ret))
}

pub static PASSWORD_GET_INFO: NativeFunction = NativeFunction {
    sig: "password_get_info(string $hash): array",
    handler: password_get_info,
};

pub static PASSWORD_HASH: NativeFunction = NativeFunction {
    sig: "password_hash(#[\\SensitiveParameter] string $password, string|int|null $algo, array $options = []): string",
    handler: password_hash,
};

pub static PASSWORD_NEEDS_REHASH: NativeFunction = NativeFunction {
    sig: "password_needs_rehash(string $hash, string|int|null $algo, array $options = []): bool",
    handler: password_needs_rehash,
};

pub static PASSWORD_VERIFY: NativeFunction = NativeFunction {
    sig: "password_verify(#[\\SensitiveParameter] string $password, string $hash): bool",
    handler: password_verify,
};

pub static PASSWORD_ALGOS: NativeFunction = NativeFunction {
    sig: "password_algos(): array",
    handler: password_algos,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &PASSWORD_GET_INFO,
        &PASSWORD_HASH,
        &PASSWORD_NEEDS_REHASH,
        &PASSWORD_VERIFY,
        &PASSWORD_ALGOS,
    ]);

    r.constant("PASSWORD_BCRYPT", "2y");
    r.constant("PASSWORD_BCRYPT_DEFAULT_COST", PASSWORD_BCRYPT_COST);
    // The `#ifdef HAVE_ARGON2LIB` constants.
    r.constant("PASSWORD_ARGON2I", "argon2i");
    r.constant("PASSWORD_ARGON2ID", "argon2id");
    r.constant("PASSWORD_ARGON2_PROVIDER", "standard");
    r.constant(
        "PASSWORD_ARGON2_DEFAULT_MEMORY_COST",
        PASSWORD_ARGON2_MEMORY_COST,
    );
    r.constant(
        "PASSWORD_ARGON2_DEFAULT_TIME_COST",
        PASSWORD_ARGON2_TIME_COST,
    );
    r.constant("PASSWORD_ARGON2_DEFAULT_THREADS", PASSWORD_ARGON2_THREADS);
    r.constant("PASSWORD_DEFAULT", "2y");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn s(v: &str) -> Value {
        Value::from(v)
    }

    fn arr(pairs: Vec<(&str, Value)>) -> Value {
        let mut a = PhpArray::new();
        for (k, v) in pairs {
            a.insert_str(k, v);
        }
        Value::from(a)
    }

    fn string(v: Value) -> String {
        match v {
            Value::String(s) => s.to_string_lossy().into_owned(),
            other => panic!("expected string, got {other:?}"),
        }
    }

    fn verify(vm: &mut TestVm, pw: &str, hash: &str) -> bool {
        match vm.call("password_verify", vec![s(pw), s(hash)]).unwrap() {
            Value::Bool(b) => b,
            other => panic!("{other:?}"),
        }
    }

    fn needs_rehash(vm: &mut TestVm, hash: &str, algo: Value, options: Option<Value>) -> bool {
        let mut args = vec![s(hash), algo];
        args.extend(options);
        match vm.call("password_needs_rehash", args).unwrap() {
            Value::Bool(b) => b,
            other => panic!("{other:?}"),
        }
    }

    fn info(vm: &mut TestVm, hash: &str) -> (Value, String, Vec<(String, i64)>) {
        let Value::Array(a) = vm.call("password_get_info", vec![s(hash)]).unwrap() else {
            panic!("not an array")
        };
        assert_eq!(a.len(), 3);
        let algo = a.get_str(b"algo").unwrap().clone();
        let name = string(a.get_str(b"algoName").unwrap().clone());
        let Some(Value::Array(opts)) = a.get_str(b"options").cloned() else {
            panic!("options")
        };
        let opts = opts
            .iter()
            .map(|(k, v)| {
                (
                    k.to_php_str().to_string_lossy().into_owned(),
                    match v {
                        Value::Int(i) => *i,
                        other => panic!("{other:?}"),
                    },
                )
            })
            .collect();
        (algo, name, opts)
    }

    const SUFFIX: &str = "MTIzNDU2Nzg5MDEyMzQ1Nej0NmcAWSLR.oP7XOR9HD/vjUuOj100y";

    /// password_default.phpt
    #[test]
    fn constants() {
        let vm = TestVm::new();
        assert!(
            matches!(vm.vm.constant("PASSWORD_DEFAULT"), Some(Value::String(s)) if s.as_bytes() == b"2y")
        );
        assert!(
            matches!(vm.vm.constant("PASSWORD_BCRYPT"), Some(Value::String(s)) if s.as_bytes() == b"2y")
        );
        assert!(matches!(
            vm.vm.constant("PASSWORD_BCRYPT_DEFAULT_COST"),
            Some(Value::Int(12))
        ));
    }

    /// password_get_info.phpt
    #[test]
    fn get_info_phpt() {
        let mut vm = TestVm::new();
        let bcrypt10 = format!("$2y$10${SUFFIX}");
        let (algo, name, opts) = info(&mut vm, &bcrypt10);
        assert_eq!(string(algo), "2y");
        assert_eq!(name, "bcrypt");
        assert_eq!(opts, vec![("cost".to_string(), 10)]);

        let (algo, name, opts) = info(&mut vm, &format!("$2y$11${SUFFIX}"));
        assert_eq!((string(algo).as_str(), name.as_str()), ("2y", "bcrypt"));
        assert_eq!(opts, vec![("cost".to_string(), 11)]);

        // Invalid length
        let (algo, name, opts) = info(&mut vm, &format!("$2y$11${}", &SUFFIX[..SUFFIX.len() - 1]));
        assert!(matches!(algo, Value::Null));
        assert_eq!(name, "unknown");
        assert!(opts.is_empty());

        // Non-bcrypt
        let (algo, name, opts) = info(&mut vm, "$1$rasmusle$rISCgZzpwk3UhDidwXvin0");
        assert!(matches!(algo, Value::Null));
        assert_eq!(name, "unknown");
        assert!(opts.is_empty());

        for cost in [4, 31] {
            let (_, name, opts) = info(&mut vm, &format!("$2y${cost:02}${SUFFIX}"));
            assert_eq!(name, "bcrypt");
            assert_eq!(opts, vec![("cost".to_string(), cost)]);
        }

        for hash in [
            format!("$2y$a0${SUFFIX}"),
            format!("$2y$10x{SUFFIX}"),
            format!("$2y$03${SUFFIX}"),
            format!("$2y$32${SUFFIX}"),
            format!("$2y${}", "9".repeat(56)),
        ] {
            let (_, name, opts) = info(&mut vm, &hash);
            assert_eq!(name, "unknown", "{hash}");
            assert!(opts.is_empty());
        }
    }

    #[test]
    fn ident_extraction() {
        assert_eq!(extract_ident(b"$2y$10$abc"), Some(&b"2y"[..]));
        assert_eq!(extract_ident(b"$$x"), Some(&b""[..]));
        assert_eq!(extract_ident(b"x2y$"), Some(&b"2y"[..]));
        assert_eq!(extract_ident(b"$2"), None);
        assert_eq!(extract_ident(b"$2y"), None);
        assert_eq!(extract_ident(b"$2y\0$"), None);
        assert_eq!(extract_ident(b"rl.3StKT.4T8M"), None);
    }

    /// password_bcrypt_errors.phpt
    #[test]
    fn bcrypt_errors_phpt() {
        let mut vm = TestVm::new();
        for (cost, msg) in [(3, "3"), (32, "32")] {
            let r = vm.call(
                "password_hash",
                vec![s("foo"), s("2y"), arr(vec![("cost", Value::Int(cost))])],
            );
            assert_eq!(
                thrown(r),
                (
                    "ValueError".to_string(),
                    format!("Invalid bcrypt cost parameter specified: {msg}")
                )
            );
        }
        let r = vm.call(
            "password_hash",
            vec![Value::from(&b"null\0password"[..]), s("2y")],
        );
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "Bcrypt password must not contain null character".to_string()
            )
        );
    }

    /// password_hash_sensitive_parameter.phpt
    #[test]
    fn hash_argument_errors() {
        let mut vm = TestVm::new();
        let r = vm.call("password_hash", vec![s("foo")]);
        assert_eq!(
            thrown(r),
            (
                "ArgumentCountError".to_string(),
                "password_hash() expects at least 2 arguments, 1 given".to_string()
            )
        );
        let r = vm.call("password_hash", vec![s("foo"), s("Invalid")]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "password_hash(): Argument #2 ($algo) must be a valid password hashing algorithm"
                    .to_string()
            )
        );
        for algo in [Value::Int(4), Value::Int(-1), s("bcrypt"), s("2Y")] {
            let r = vm.call("password_hash", vec![s("foo"), algo]);
            assert_eq!(thrown(r).0, "ValueError");
        }
    }

    /// password_bcrypt_short.phpt
    #[test]
    fn bcrypt_short_phpt() {
        let mut vm = TestVm::new();
        assert!(!verify(&mut vm, "foo", "$2"));
    }

    /// password_verify.phpt
    #[test]
    fn verify_phpt() {
        let mut vm = TestVm::new();
        // password_verify(123, 123) is coerced to strings.
        assert!(matches!(
            vm.call("password_verify", vec![Value::Int(123), Value::Int(123)]),
            Ok(Value::Bool(false))
        ));
        assert!(!verify(&mut vm, "foo", "$2a$07$usesomesillystringforsalt$"));
        let h = "$2a$07$usesomesillystringfore2uDLvp1Ii2e./U9C8sBjqp8I90dH6hi";
        assert!(!verify(&mut vm, "rasmusler", h));
        assert!(verify(&mut vm, "rasmuslerdorf", h));

        let r = vm.call("password_verify", vec![s("foo"), Value::Null]);
        assert!(matches!(r, Ok(Value::Bool(false))));
        assert!(vm.output().contains(
            "Deprecated: password_verify(): Passing null to parameter #2 ($hash) of type string is deprecated in"
        ));

        assert!(verify(&mut vm, "rasmuslerdorf", "rl.3StKT.4T8M"));
        assert!(!verify(&mut vm, "foo", "$1"));
    }

    #[test]
    fn verify_other_crypt_formats() {
        let mut vm = TestVm::new();
        for (pw, hash) in [
            ("rasmuslerdorf", "rl.3StKT.4T8M"),
            ("rasmuslerdorf", "_J9..rasmBYk8r9AiWNc"),
            ("rasmuslerdorf", "$1$rasmusle$rISCgZzpwk3UhDidwXvin0"),
            (
                "Hello world!",
                "$5$saltstring$5B8vYYiY.CVt1RlTTf8KbXBH3hsxY/GNooZaBBGWEc5",
            ),
            (
                "Hello world!",
                "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1",
            ),
            (
                "U*U",
                "$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW",
            ),
        ] {
            assert!(verify(&mut vm, pw, hash), "{hash}");
            assert!(!verify(&mut vm, "wrong", hash), "{hash}");
        }
        // Failure strings and malformed hashes never verify.
        assert!(!verify(&mut vm, "", "*0"));
        assert!(!verify(&mut vm, "", "*1"));
        assert!(!verify(&mut vm, "", ""));
        assert!(!verify(&mut vm, "", "short"));
    }

    /// password_needs_rehash.phpt
    #[test]
    fn needs_rehash_phpt() {
        let mut vm = TestVm::new();
        // Invalid hash, always rehash
        assert!(needs_rehash(&mut vm, "", s("2y"), None));
        assert!(needs_rehash(&mut vm, "", Value::Int(1), None));
        assert!(needs_rehash(&mut vm, "", s("2y"), None));
        // Valid, as it's an unknown algorithm
        assert!(needs_rehash(&mut vm, "", s("2y"), None));
        assert!(needs_rehash(&mut vm, "", Value::Int(0), None));
        assert!(needs_rehash(&mut vm, "", Value::Null, None));

        let h10 = format!("$2y$10${SUFFIX}");
        let cost = |c: Value| Some(arr(vec![("cost", c)]));
        // Valid with cost the same
        assert!(!needs_rehash(&mut vm, &h10, s("2y"), cost(Value::Int(10))));
        // Valid with cost the same, additional params
        assert!(!needs_rehash(
            &mut vm,
            &h10,
            s("2y"),
            Some(arr(vec![("cost", Value::Int(10)), ("foo", Value::Int(3))]))
        ));
        // Different (lower) cost
        assert!(needs_rehash(&mut vm, &h10, s("2y"), cost(Value::Int(0))));
        // Different (higher) cost
        assert!(needs_rehash(&mut vm, &h10, s("2y"), cost(Value::Int(11))));
        // Valid with cost the default
        let h12 = format!("$2y$12${SUFFIX}");
        assert!(!needs_rehash(&mut vm, &h12, s("2y"), None));
        // 'foo' is cast to 0...
        assert!(needs_rehash(&mut vm, &h10, s("2y"), cost(s("foo"))));

        // Valid cost boundaries
        let cases: [(&str, String, i64, bool); 6] = [
            ("valid lower boundary", format!("$2y$04${SUFFIX}"), 4, false),
            (
                "valid upper boundary",
                format!("$2y$31${SUFFIX}"),
                31,
                false,
            ),
            ("non-digit", format!("$2y$a0${SUFFIX}"), 12, true),
            ("malformed separator", format!("$2y$10x{SUFFIX}"), 10, true),
            ("below range", format!("$2y$03${SUFFIX}"), 3, true),
            ("above range", format!("$2y$32${SUFFIX}"), 32, true),
        ];
        for (desc, hash, c, expected) in cases {
            assert_eq!(
                needs_rehash(&mut vm, &hash, s("2y"), cost(Value::Int(c))),
                expected,
                "{desc}"
            );
        }

        // CRYPT_MD5 / CRYPT_SHA512 hashes always need a rehash to the default.
        let md5 = string(vm.call("crypt", vec![s("Example"), s("$1$")]).unwrap());
        assert!(needs_rehash(&mut vm, &md5, s("2y"), None));
        let sha = string(
            vm.call("crypt", vec![s("Example"), s("$6$rounds=5000$aa$")])
                .unwrap(),
        );
        assert!(needs_rehash(&mut vm, &sha, s("2y"), None));

        // Unknown new algorithm: never prompt to rehash.
        assert!(!needs_rehash(&mut vm, &h10, Value::Int(99), None));
        assert!(!needs_rehash(&mut vm, &h10, s("nope"), None));
    }

    /// password_hash.phpt
    #[test]
    fn hash_phpt() {
        let mut vm = TestVm::new();
        let hash = string(vm.call("password_hash", vec![s("foo"), s("2y")]).unwrap());
        assert_eq!(hash.len(), 60);
        assert!(hash.starts_with("$2y$12$"));
        // The algorithm selectors of the phpt (cost lowered to keep the test fast).
        let cost4 = || arr(vec![("cost", Value::Int(4))]);
        for algo in [Value::Null, s("2y"), Value::Int(1), Value::Int(0)] {
            let hash = string(
                vm.call("password_hash", vec![s("foo"), algo, cost4()])
                    .unwrap(),
            );
            assert!(hash.starts_with("$2y$04$"), "{hash}");
            assert_eq!(
                string(vm.call("crypt", vec![s("foo"), s(&hash)]).unwrap()),
                hash
            );
            assert!(verify(&mut vm, "foo", &hash));
            assert!(!verify(&mut vm, "bar", &hash));
            assert!(!needs_rehash(&mut vm, &hash, s("2y"), Some(cost4())));
        }
    }

    #[test]
    fn hash_options_and_randomness() {
        let mut vm = TestVm::new();
        let opts = Some(arr(vec![("cost", Value::Int(5))]));
        let mut seen = std::collections::HashSet::new();
        for _ in 0..4 {
            let mut args = vec![s("pw"), s("2y")];
            args.extend(opts.clone());
            let hash = string(vm.call("password_hash", args).unwrap());
            assert_eq!(hash.len(), 60);
            assert!(hash.starts_with("$2y$05$"), "{hash}");
            assert!(
                hash[7..29]
                    .bytes()
                    .all(|c| c == b'.' || c == b'/' || c.is_ascii_alphanumeric()),
                "{hash}"
            );
            assert!(verify(&mut vm, "pw", &hash));
            assert!(seen.insert(hash));
        }
        // A numeric string cost is converted like zval_get_long().
        let hash = string(
            vm.call(
                "password_hash",
                vec![s("pw"), s("2y"), arr(vec![("cost", s("4abc"))])],
            )
            .unwrap(),
        );
        assert!(hash.starts_with("$2y$04$"));
    }

    /// password_removed_salt_option.phpt
    #[test]
    fn removed_salt_option_phpt() {
        let mut vm = TestVm::new();
        let hash = string(
            vm.call(
                "password_hash",
                vec![
                    s("rasmuslerdorf"),
                    s("2y"),
                    arr(vec![
                        ("cost", Value::Int(7)),
                        ("salt", s("usesomesillystringforsalt")),
                    ]),
                ],
            )
            .unwrap(),
        );
        assert_eq!(hash.len(), 60);
        assert!(hash.starts_with("$2y$07$"));
        let out = vm.output();
        assert!(
            out.contains("Warning: password_hash(): The \"salt\" option has been ignored, since providing a custom salt is no longer supported in /t.php on line 3"),
            "{out}"
        );

        let hash = string(
            vm.call(
                "password_hash",
                vec![
                    s("test"),
                    s("2y"),
                    arr(vec![
                        ("cost", Value::Int(4)),
                        ("salt", Value::from(&b"123456789012345678901\0"[..])),
                    ]),
                ],
            )
            .unwrap(),
        );
        assert_eq!(hash.len(), 60);
        assert!(vm.output().contains("The \"salt\" option has been ignored"));
    }

    #[test]
    fn algos() {
        let mut vm = TestVm::new();
        let Value::Array(a) = vm.call("password_algos", vec![]).unwrap() else {
            panic!()
        };
        let names: Vec<String> = a.values().map(|v| string(v.clone())).collect();
        assert_eq!(names, ["2y", "argon2i", "argon2id"]);
    }

    #[test]
    fn salt_generation() {
        let mut vm = TestVm::new();
        // 22 characters from 17 random bytes; '+' becomes '.'.
        let salt = make_salt_with(&mut vm.vm, 22, |n| {
            assert_eq!(n, 17);
            Ok(vec![0xfb; n])
        })
        .unwrap();
        assert_eq!(salt, b"./v7./v7./v7./v7./v7./");
        assert_eq!(salt.len(), 22);
        assert!(salt.contains(&b'.'));

        // php_random_bytes_throw() failure: ValueError over a RandomException.
        let err = make_salt_with(&mut vm.vm, 22, |_| {
            Err("Cannot open /dev/urandom: boom".into())
        })
        .unwrap_err();
        let Flow::Exception(e) = err else { panic!() };
        assert_eq!(e.class().name.to_string(), "ValueError");
        assert_eq!(Vm::exception_message(&e), "Unable to generate salt");
        let Some(Value::Object(prev)) = e.get_prop(b"previous") else {
            panic!("no previous exception")
        };
        assert_eq!(
            Vm::exception_message(&prev),
            "Cannot open /dev/urandom: boom"
        );

        let err = make_salt_with(&mut vm.vm, usize::MAX / 2, |_| unreachable!()).unwrap_err();
        let Flow::Exception(e) = err else { panic!() };
        assert_eq!(
            Vm::exception_message(&e),
            "Length is too large to safely generate"
        );
    }

    #[test]
    fn base64() {
        assert_eq!(base64_encode(b""), b"");
        assert_eq!(base64_encode(b"f"), b"Zg==");
        assert_eq!(base64_encode(b"fo"), b"Zm8=");
        assert_eq!(base64_encode(b"foo"), b"Zm9v");
        assert_eq!(base64_encode(b"foobar"), b"Zm9vYmFy");
    }

    #[test]
    fn random_bytes_work() {
        let a = random_bytes(32).unwrap();
        let b = random_bytes(32).unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    // ----- argon2 -----------------------------------------------------------

    const ARGON2I_HASH: &str = "$argon2i$v=19$m=65536,t=3,p=1$OEVjWWs2Z3YvWlNZQ0ZmNw$JKin7ahjmh8JYvMyFcXri0Ss/Uvd3uYpD7MG6C/5Cy0";
    const ARGON2ID_HASH: &str = "$argon2id$v=19$m=1024,t=2,p=2$WS90MHJhd3AwSC5xTDJpZg$8tn2DaIJR2/UX4Cjcy2t3EZaLDL/qh+NbLQAOvTmdAg";

    /// Small, fast parameters for the tests that do not need the defaults.
    fn small_argon2() -> Value {
        arr(vec![
            ("memory_cost", Value::Int(64)),
            ("time_cost", Value::Int(2)),
            ("threads", Value::Int(2)),
        ])
    }

    #[test]
    fn argon2_constants() {
        let vm = TestVm::new();
        let c = |n: &str| vm.vm.constant(n).unwrap_or_else(|| panic!("{n} missing"));
        assert!(matches!(c("PASSWORD_ARGON2I"), Value::String(s) if s.as_bytes() == b"argon2i"));
        assert!(matches!(c("PASSWORD_ARGON2ID"), Value::String(s) if s.as_bytes() == b"argon2id"));
        assert!(
            matches!(c("PASSWORD_ARGON2_PROVIDER"), Value::String(s) if s.as_bytes() == b"standard")
        );
        assert!(matches!(
            c("PASSWORD_ARGON2_DEFAULT_MEMORY_COST"),
            Value::Int(65536)
        ));
        assert!(matches!(
            c("PASSWORD_ARGON2_DEFAULT_TIME_COST"),
            Value::Int(4)
        ));
        assert!(matches!(
            c("PASSWORD_ARGON2_DEFAULT_THREADS"),
            Value::Int(1)
        ));
    }

    /// password_get_info_argon2.phpt
    #[test]
    fn argon2_get_info_phpt() {
        let mut vm = TestVm::new();
        let (algo, name, opts) = info(
            &mut vm,
            "$argon2i$v=19$m=65536,t=3,p=1$SWhIcG5MT21Pc01PbWdVZw$WagZELICsz7jlqOR2YzoEVTWb2oOX1tYdnhZYXxptbU",
        );
        assert_eq!(
            (string(algo).as_str(), name.as_str()),
            ("argon2i", "argon2i")
        );
        assert_eq!(
            opts,
            vec![
                ("memory_cost".to_string(), 65536),
                ("time_cost".to_string(), 3),
                ("threads".to_string(), 1)
            ]
        );
        let (algo, name, opts) = info(
            &mut vm,
            "$argon2id$v=19$m=1024,t=2,p=2$Zng1U1RHS0h1aUJjbGhPdA$ajQnG5s01Ws1ad8xv+1qGfXF8mYxxWdyul5rBpomuZQ",
        );
        assert_eq!(
            (string(algo).as_str(), name.as_str()),
            ("argon2id", "argon2id")
        );
        assert_eq!(
            opts,
            vec![
                ("memory_cost".to_string(), 1024),
                ("time_cost".to_string(), 2),
                ("threads".to_string(), 2)
            ]
        );
    }

    /// `extract_argon2_parameters()` keeps the values sscanf() did not reach.
    #[test]
    fn argon2_partial_parameters() {
        let mut vm = TestVm::new();
        let defaults = |m, t, p| {
            vec![
                ("memory_cost".to_string(), m),
                ("time_cost".to_string(), t),
                ("threads".to_string(), p),
            ]
        };
        // Complete.
        assert_eq!(
            info(&mut vm, "$argon2i$v=19$m=8,t=1,p=3$x$y").2,
            defaults(8, 1, 3)
        );
        // Stops after "t": threads keeps its default.
        assert_eq!(
            info(&mut vm, "$argon2i$v=19$m=1024,t=2,x=3$x$y").2,
            defaults(1024, 2, 1)
        );
        // Stops after "m".
        assert_eq!(
            info(&mut vm, "$argon2id$v=19$m=7;t=2,p=3$x$y").2,
            defaults(7, 4, 1)
        );
        // No "v=" at all: nothing is read.
        assert_eq!(
            info(&mut vm, "$argon2id$m=7,t=2,p=3$x$y").2,
            defaults(65536, 4, 1)
        );
        // Too short for php-src to even look at: defaults.
        assert_eq!(info(&mut vm, "$argon2i$").2, defaults(65536, 4, 1));
        // %ld skips white space and accepts signs.
        assert_eq!(
            info(&mut vm, "$argon2i$v= 19$m= 16,t=+3,p=-1$x$y").2,
            defaults(16, 3, -1)
        );

        // needs_rehash() starts from zeros when the hash cannot be scanned.
        let defaults_opts = Some(arr(vec![
            ("memory_cost", Value::Int(0)),
            ("time_cost", Value::Int(0)),
            ("threads", Value::Int(0)),
        ]));
        assert!(!needs_rehash(
            &mut vm,
            "$argon2i$garbage!!",
            s("argon2i"),
            defaults_opts
        ));
        assert!(needs_rehash(
            &mut vm,
            "$argon2i$garbage!!",
            s("argon2i"),
            None
        ));
    }

    /// password_hash_argon2.phpt (with small parameters).
    #[test]
    fn argon2_hash_and_verify_phpt() {
        let mut vm = TestVm::new();
        let password = "the password for testing 12345!";
        for (algo, ident) in [
            (s("argon2i"), "argon2i"),
            (Value::Int(2), "argon2i"),
            (s("argon2id"), "argon2id"),
            (Value::Int(3), "argon2id"),
        ] {
            let hash = string(
                vm.call("password_hash", vec![s(password), algo, small_argon2()])
                    .unwrap(),
            );
            assert!(
                hash.starts_with(&format!("${ident}$v=19$m=64,t=2,p=2$")),
                "{hash}"
            );
            assert!(verify(&mut vm, password, &hash));
            assert!(!verify(&mut vm, "other", &hash));
            let (a, name, opts) = info(&mut vm, &hash);
            assert_eq!(string(a), ident);
            assert_eq!(name, ident);
            assert_eq!(
                opts,
                vec![
                    ("memory_cost".to_string(), 64),
                    ("time_cost".to_string(), 2),
                    ("threads".to_string(), 2)
                ]
            );
            // bug75221.phpt: no NUL at the end.
            assert!(!hash.ends_with('\0'));
            // The salt is 16 characters, base64 encoded without padding.
            assert_eq!(hash.split('$').nth(4).unwrap().len(), 22);
        }
    }

    /// The default parameters (64 MiB, 4 passes, 1 lane).
    #[test]
    fn argon2_default_parameters() {
        let mut vm = TestVm::new();
        let hash = string(
            vm.call("password_hash", vec![s("test"), s("argon2id")])
                .unwrap(),
        );
        assert!(
            hash.starts_with("$argon2id$v=19$m=65536,t=4,p=1$"),
            "{hash}"
        );
        assert_eq!(hash.len(), 97);
        assert!(verify(&mut vm, "test", &hash));
        assert!(!needs_rehash(&mut vm, &hash, s("argon2id"), None));
    }

    /// password_hash_error_argon2.phpt
    #[test]
    fn argon2_errors_phpt() {
        let mut vm = TestVm::new();
        for algo in ["argon2i", "argon2id"] {
            for (opt, msg) in [
                (
                    "memory_cost",
                    "Memory cost is outside of allowed memory range",
                ),
                ("time_cost", "Time cost is outside of allowed time range"),
                ("threads", "Invalid number of threads"),
            ] {
                let r = vm.call(
                    "password_hash",
                    vec![s("test"), s(algo), arr(vec![(opt, Value::Int(0))])],
                );
                assert_eq!(
                    thrown(r),
                    ("ValueError".to_string(), msg.to_string()),
                    "{algo} {opt}"
                );
            }
        }
        // Negative and too large values (size_t in php-src).
        for (opt, v, msg) in [
            (
                "memory_cost",
                -1,
                "Memory cost is outside of allowed memory range",
            ),
            (
                "memory_cost",
                7,
                "Memory cost is outside of allowed memory range",
            ),
            (
                "memory_cost",
                1 << 32,
                "Memory cost is outside of allowed memory range",
            ),
            (
                "time_cost",
                -5,
                "Time cost is outside of allowed time range",
            ),
            (
                "time_cost",
                1 << 32,
                "Time cost is outside of allowed time range",
            ),
            ("threads", -1, "Invalid number of threads"),
            ("threads", 0x100_0000, "Invalid number of threads"),
        ] {
            let r = vm.call(
                "password_hash",
                vec![s("test"), s("argon2i"), arr(vec![(opt, Value::Int(v))])],
            );
            assert_eq!(
                thrown(r),
                ("ValueError".to_string(), msg.to_string()),
                "{opt}={v}"
            );
        }
        // Errors reported by libargon2 itself.
        let r = vm.call(
            "password_hash",
            vec![
                s("test"),
                s("argon2i"),
                arr(vec![
                    ("memory_cost", Value::Int(8)),
                    ("threads", Value::Int(2)),
                ]),
            ],
        );
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "Memory cost is too small".to_string()
            )
        );
    }

    /// password_verify_argon2.phpt
    #[test]
    fn argon2_verify_phpt() {
        let mut vm = TestVm::new();
        assert!(verify(&mut vm, "test", ARGON2I_HASH));
        assert!(!verify(&mut vm, "argon2", ARGON2I_HASH));
        assert!(verify(&mut vm, "test", ARGON2ID_HASH));
        assert!(!verify(&mut vm, "argon2id", ARGON2ID_HASH));
        // Truncated or corrupted hashes never verify.
        assert!(!verify(
            &mut vm,
            "test",
            &ARGON2I_HASH[..ARGON2I_HASH.len() - 1]
        ));
        assert!(!verify(
            &mut vm,
            "test",
            &ARGON2I_HASH.replace("m=65536", "m=65535")
        ));
        assert!(!verify(&mut vm, "test", "$argon2i$"));
    }

    /// password_needs_rehash_argon2.phpt (on precomputed hashes: only the
    /// parameters in the hash string matter).
    #[test]
    fn argon2_needs_rehash_phpt() {
        let mut vm = TestVm::new();
        let hash = |ty: &str| {
            format!("${ty}$v=19$m=65536,t=4,p=1$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG")
        };
        for ty in ["argon2i", "argon2id"] {
            let h = hash(ty);
            assert!(!needs_rehash(&mut vm, &h, s(ty), None));
            assert!(needs_rehash(
                &mut vm,
                &h,
                s(ty),
                Some(arr(vec![("memory_cost", Value::Int(65536 * 2))]))
            ));
            assert!(needs_rehash(
                &mut vm,
                &h,
                s(ty),
                Some(arr(vec![("time_cost", Value::Int(5))]))
            ));
            assert!(needs_rehash(
                &mut vm,
                &h,
                s(ty),
                Some(arr(vec![("threads", Value::Int(2))]))
            ));
            // Same values explicitly.
            assert!(!needs_rehash(
                &mut vm,
                &h,
                s(ty),
                Some(arr(vec![
                    ("memory_cost", Value::Int(65536)),
                    ("time_cost", Value::Int(4)),
                    ("threads", Value::Int(1))
                ]))
            ));
        }
        // A different algorithm always needs a rehash.
        assert!(needs_rehash(&mut vm, &hash("argon2i"), s("argon2id"), None));
        assert!(needs_rehash(
            &mut vm,
            &hash("argon2id"),
            Value::Int(2),
            None
        ));
        assert!(needs_rehash(&mut vm, &hash("argon2i"), s("2y"), None));
        assert!(needs_rehash(
            &mut vm,
            &format!("$2y$10${SUFFIX}"),
            s("argon2i"),
            None
        ));
    }

    /// password_removed_salt_option.phpt applies to argon2 as well.
    #[test]
    fn argon2_removed_salt_option() {
        let mut vm = TestVm::new();
        let mut opts = PhpArray::new();
        opts.insert_str("memory_cost", Value::Int(16));
        opts.insert_str("time_cost", Value::Int(1));
        opts.insert_str("salt", s("usesomesillystringforsalt"));
        let hash = string(
            vm.call(
                "password_hash",
                vec![s("pw"), s("argon2i"), Value::from(opts)],
            )
            .unwrap(),
        );
        assert!(hash.starts_with("$argon2i$v=19$m=16,t=1,p=1$"));
        assert!(vm.output().contains("The \"salt\" option has been ignored"));
    }

    #[test]
    fn argon2_binary_passwords() {
        let mut vm = TestVm::new();
        // Unlike bcrypt, argon2 hashes the whole password including NULs.
        let pw = Value::from(&b"null\0password"[..]);
        let hash = string(
            vm.call(
                "password_hash",
                vec![pw.clone(), s("argon2id"), small_argon2()],
            )
            .unwrap(),
        );
        assert!(matches!(
            vm.call("password_verify", vec![pw, s(&hash)]),
            Ok(Value::Bool(true))
        ));
        assert!(!verify(&mut vm, "null", &hash));
    }
}
