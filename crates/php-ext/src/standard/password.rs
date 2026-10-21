//! ext/standard/password.c — `password_hash()` and friends.

use php_runtime::{Args, Flow, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::crypt::{php_crypt, php_safe_bcmp};

/// `PHP_PASSWORD_BCRYPT_COST`.
pub const PASSWORD_BCRYPT_COST: i64 = 12;

/// The registered password hashing algorithms (`php_password_algos`), in
/// registration order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Algo {
    Bcrypt,
}

/// Registered algorithms in `php_password_algo_register()` order.
const ALGOS: &[Algo] = &[Algo::Bcrypt];

impl Algo {
    /// The registry key (`ident`), as in `$2y$...`.
    fn ident(self) -> &'static str {
        match self {
            Algo::Bcrypt => "2y",
        }
    }

    /// `php_password_algo.name`.
    fn name(self) -> &'static str {
        match self {
            Algo::Bcrypt => "bcrypt",
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
        }
    }

    /// `php_password_algo.needs_rehash`.
    fn needs_rehash(self, vm: &mut Vm, hash: &[u8], options: &PhpArray) -> PResult<bool> {
        match self {
            Algo::Bcrypt => bcrypt_needs_rehash(vm, hash, options),
        }
    }

    /// `php_password_algo.verify`.
    fn verify(self, password: &[u8], hash: &[u8]) -> bool {
        match self {
            Algo::Bcrypt => bcrypt_verify(password, hash),
        }
    }

    /// `php_password_algo.hash`: `Ok(None)` is php-src's NULL return without a
    /// pending exception.
    fn hash(self, vm: &mut Vm, password: &[u8], options: &PhpArray) -> PResult<Option<Vec<u8>>> {
        match self {
            Algo::Bcrypt => bcrypt_hash(vm, password, options),
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
        assert_eq!(names[0], "2y");
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
}
