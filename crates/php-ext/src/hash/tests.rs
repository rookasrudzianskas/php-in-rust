//! Unit tests for ext/hash, with expectations taken from `ext/hash/tests/*.phpt`.
//!
//! The file and stream variants (`hash_file`, `hash_hmac_file`,
//! `hash_update_file`, `hash_update_stream`) depend on the stream layer and are
//! `#[ignore]`d until it exists.

use php_runtime::class::fn_flags;
use php_runtime::{Flow, ObjRef, PResult, PhpArray, ResRef, Value};

use crate::testing::{TestVm, thrown};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn s(v: &str) -> Value {
    Value::str(v)
}

fn text(v: &Value) -> String {
    String::from_utf8_lossy(v.as_str().expect("a string").as_bytes()).into_owned()
}

fn hex(v: &Value) -> String {
    let bytes = v.as_str().expect("a string").as_bytes();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ok(t: &mut TestVm, name: &str, args: Vec<Value>) -> Value {
    match t.call(name, args) {
        Ok(v) => v,
        Err(Flow::Exception(e)) => panic!(
            "{name}() threw {}: {}",
            e.class().name,
            php_runtime::Vm::exception_message(&e)
        ),
        Err(other) => panic!("{name}() failed with {other:?}"),
    }
}

fn hash_hex(t: &mut TestVm, algo: &str, data: &str) -> String {
    text(&ok(t, "hash", vec![s(algo), s(data)]))
}

fn obj(v: Value) -> ObjRef {
    v.as_object().expect("an object").clone()
}

fn init(t: &mut TestVm, algo: &str) -> ObjRef {
    obj(ok(t, "hash_init", vec![s(algo)]))
}

fn update(t: &mut TestVm, ctx: &ObjRef, data: &str) {
    ok(t, "hash_update", vec![Value::Object(ctx.clone()), s(data)]);
}

fn finish(t: &mut TestVm, ctx: &ObjRef) -> String {
    text(&ok(t, "hash_final", vec![Value::Object(ctx.clone())]))
}

fn assert_thrown(r: PResult<Value>, class: &str, message: &str) {
    let (c, m) = thrown(r);
    assert_eq!((c.as_str(), m.as_str()), (class, message));
}

fn opts(pairs: Vec<(&str, Value)>) -> Value {
    Value::array(PhpArray::from_pairs(pairs))
}

fn blank_context(t: &mut TestVm) -> ObjRef {
    let ce = t.vm.find_class(b"HashContext").expect("HashContext class");
    t.vm.instantiate(&ce).expect("instantiate")
}

fn serialize(t: &mut TestVm, ctx: &ObjRef) -> PResult<Value> {
    t.vm.call_method(ctx, b"__serialize", vec![])
}

fn unserialize(t: &mut TestVm, data: &Value) -> PResult<ObjRef> {
    let ctx = blank_context(t);
    t.vm.call_method(&ctx, b"__unserialize", vec![data.clone()])?;
    Ok(ctx)
}

fn array_of(v: &Value) -> PhpArray {
    (**v.as_array().expect("an array")).clone()
}

const SAMPLE: &str =
    "This is a sample string used to test the hash_hmac function with various hashing algorithms";

// ---------------------------------------------------------------------------
// hash(), hash_algos(), hash_hmac_algos()
// ---------------------------------------------------------------------------

#[test]
fn hash_known_vectors() {
    let mut t = TestVm::new();
    for (algo, data, expected) in [
        ("md5", "abc", "900150983cd24fb0d6963f7d28e17f72"),
        ("sha1", "abc", "a9993e364706816aba3e25717850c26c9cd0d89d"),
        (
            "sha256",
            "abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            "md5",
            "The quick brown fox jumped over the lazy dog.",
            "5c6ffbdd40d9556b73a21e63c3e0e904",
        ),
        (
            "crc32b",
            "The quick brown fox jumped over the lazy dog.",
            "82a34642",
        ),
        // The algorithm name is case-insensitive.
        ("MD5", "abc", "900150983cd24fb0d6963f7d28e17f72"),
        (
            "Sha256",
            "",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
    ] {
        assert_eq!(hash_hex(&mut t, algo, data), expected, "{algo}({data:?})");
    }
}

#[test]
fn hash_binary_flag_and_named_options() {
    let mut t = TestVm::new();
    let raw = ok(&mut t, "hash", vec![s("md5"), s("abc"), Value::Bool(true)]);
    assert_eq!(hex(&raw), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(raw.as_str().unwrap().len(), 16);

    let named =
        t.vm.call_native_named(
            "hash",
            vec![s("md5"), s("abc")],
            vec![("binary", Value::Bool(true))],
        )
        .unwrap();
    assert_eq!(hex(&named), "900150983cd24fb0d6963f7d28e17f72");
}

/// hash_error.phpt
#[test]
fn hash_rejects_unknown_algorithms() {
    let mut t = TestVm::new();
    assert_thrown(
        t.call("hash", vec![s("foo"), s("")]),
        "ValueError",
        "hash(): Argument #1 ($algo) must be a valid hashing algorithm",
    );
    // Not valid UTF-8, so certainly not an algorithm.
    assert_thrown(
        t.call("hash", vec![Value::bytes(b"md5\xff"), s("")]),
        "ValueError",
        "hash(): Argument #1 ($algo) must be a valid hashing algorithm",
    );
}

/// hash_algos.phpt / hash_hmac_algos.phpt
#[test]
fn hash_algos_lists() {
    let mut t = TestVm::new();
    let all = ok(&mut t, "hash_algos", vec![]);
    let names: Vec<String> = all.as_array().unwrap().values().map(text).collect();
    assert_eq!(names.len(), 60);
    assert_eq!(&names[..4], ["md2", "md4", "md5", "sha1"]);
    assert_eq!(names.last().unwrap(), "haval256,5");
    assert!(names.iter().any(|n| n == "xxh128"));

    let hmac = ok(&mut t, "hash_hmac_algos", vec![]);
    let names: Vec<String> = hmac.as_array().unwrap().values().map(text).collect();
    assert_eq!(names.len(), 44);
    assert_eq!(names[0], "md2");
    assert!(names.iter().any(|n| n == "snefru256"));
    for non_crypto in ["crc32", "adler32", "xxh3", "murmur3a", "joaat"] {
        assert!(!names.iter().any(|n| n == non_crypto), "{non_crypto}");
    }
    assert!(all.as_array().unwrap().is_list());
}

// ---------------------------------------------------------------------------
// hash_hmac(), hash_init() with HASH_HMAC
// ---------------------------------------------------------------------------

/// hash_hmac_basic.phpt
#[test]
fn hash_hmac_known_vectors() {
    let mut t = TestVm::new();
    for (algo, expected) in [
        ("md5", "2a632783e2812cf23de100d7d6a463ae"),
        ("sha1", "5bfdb62b97e2c987405463e9f7c193139c0e1fd0"),
        (
            "sha256",
            "49bde3496b9510a17d0edd8a4b0ac70148e32a1d51e881ec76faa96534125838",
        ),
    ] {
        let mac = ok(&mut t, "hash_hmac", vec![s(algo), s(SAMPLE), s("secret")]);
        assert_eq!(text(&mac), expected, "{algo}");
        let raw = ok(
            &mut t,
            "hash_hmac",
            vec![s(algo), s(SAMPLE), s("secret"), Value::Bool(true)],
        );
        assert_eq!(hex(&raw), expected, "{algo} (raw)");
    }
}

/// hmac-md5.phpt (RFC 2104 vectors)
#[test]
fn hmac_md5_rfc2104() {
    let mut t = TestVm::new();
    let hmac = Value::Int(1);

    let ctx = obj(ok(
        &mut t,
        "hash_init",
        vec![s("md5"), hmac.clone(), Value::bytes(&[0x0b; 16])],
    ));
    update(&mut t, &ctx, "Hi There");
    assert_eq!(finish(&mut t, &ctx), "9294727a3638bb1c13f48ef8158bfc9d");

    let ctx = obj(ok(&mut t, "hash_init", vec![s("md5"), hmac, s("Jefe")]));
    update(&mut t, &ctx, "what do ya want for nothing?");
    assert_eq!(finish(&mut t, &ctx), "750c783e6ab0b503eaa86e310a5db738");

    let mac = ok(
        &mut t,
        "hash_hmac",
        vec![
            s("md5"),
            Value::bytes(&[0xDD; 50]),
            Value::bytes(&[0xAA; 16]),
        ],
    );
    assert_eq!(text(&mac), "56be34521d144c88dbb8c733f0e8b3f6");
}

/// hash_hmac_error.phpt and hash_hmac_file_error.phpt
#[test]
fn hash_hmac_rejects_bad_algorithms() {
    let mut t = TestVm::new();
    for algo in ["foo", "crc32"] {
        assert_thrown(
            t.call("hash_hmac", vec![s(algo), s("data"), s("key")]),
            "ValueError",
            "hash_hmac(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm",
        );
        assert_thrown(
            t.call(
                "hash_hmac_file",
                vec![s(algo), s("file"), s("key"), Value::Bool(true)],
            ),
            "ValueError",
            "hash_hmac_file(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm",
        );
    }
}

/// hash_file_error.phpt, hash_hmac_file_error.phpt: the NUL check precedes any I/O.
#[test]
fn file_functions_reject_null_bytes() {
    let mut t = TestVm::new();
    assert_thrown(
        t.call("hash_file", vec![s("md5"), Value::bytes(b"a\0b")]),
        "ValueError",
        "hash_file(): Argument #2 ($filename) must not contain any null bytes",
    );
    assert_thrown(
        t.call("hash_file", vec![s("foobar"), s("x")]),
        "ValueError",
        "hash_file(): Argument #1 ($algo) must be a valid hashing algorithm",
    );
    assert_thrown(
        t.call(
            "hash_hmac_file",
            vec![
                s("md5"),
                Value::bytes(b"a\0b"),
                s("secret"),
                Value::Bool(true),
            ],
        ),
        "ValueError",
        "hash_hmac_file(): Argument #2 ($filename) must not contain any null bytes",
    );
}

/// hash_init_error.phpt
#[test]
fn hash_init_errors() {
    let mut t = TestVm::new();
    assert_thrown(
        t.call("hash_init", vec![s("dummy")]),
        "ValueError",
        "hash_init(): Argument #1 ($algo) must be a valid hashing algorithm",
    );
    assert_thrown(
        t.call("hash_init", vec![s("crc32"), Value::Int(1)]),
        "ValueError",
        "hash_init(): Argument #1 ($algo) must be a cryptographic hashing algorithm if HMAC is requested",
    );
    assert_thrown(
        t.call("hash_init", vec![s("md5"), Value::Int(1)]),
        "ValueError",
        "hash_init(): Argument #3 ($key) must not be empty when HMAC is requested",
    );
    t.output();
    assert_thrown(
        t.call("hash_init", vec![s("md5"), Value::Int(1), Value::Null]),
        "ValueError",
        "hash_init(): Argument #3 ($key) must not be empty when HMAC is requested",
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: hash_init(): Passing null to parameter #3 ($key) of type string is deprecated in /t.php on line 3\n"
    );
}

// ---------------------------------------------------------------------------
// hash_init(), hash_update(), hash_final(), hash_copy()
// ---------------------------------------------------------------------------

#[test]
fn incremental_hashing_matches_one_shot() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "sha256");
    assert!(matches!(
        ok(
            &mut t,
            "hash_update",
            vec![Value::Object(ctx.clone()), s("The quick brown fox ")]
        ),
        Value::Bool(true)
    ));
    update(&mut t, &ctx, "jumped over the lazy dog.");
    assert_eq!(
        finish(&mut t, &ctx),
        hash_hex(
            &mut t,
            "sha256",
            "The quick brown fox jumped over the lazy dog."
        )
    );
}

#[test]
fn hash_final_binary_flag() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "md5");
    update(&mut t, &ctx, "abc");
    let raw = ok(
        &mut t,
        "hash_final",
        vec![Value::Object(ctx), Value::Bool(true)],
    );
    assert_eq!(raw.as_str().unwrap().len(), 16);
    assert_eq!(hex(&raw), "900150983cd24fb0d6963f7d28e17f72");
}

/// reuse.phpt, gh12186_1.phpt
#[test]
fn finalized_contexts_are_rejected() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "md5");
    finish(&mut t, &ctx);
    let arg = || Value::Object(ctx.clone());
    assert_thrown(
        t.call("hash_update", vec![arg(), s("foo")]),
        "TypeError",
        "hash_update(): Argument #1 ($context) must be a valid, non-finalized HashContext",
    );
    assert_thrown(
        t.call("hash_final", vec![arg()]),
        "TypeError",
        "hash_final(): Argument #1 ($context) must be a valid, non-finalized HashContext",
    );
    assert_thrown(
        t.call("hash_copy", vec![arg()]),
        "TypeError",
        "hash_copy(): Argument #1 ($context) must be a valid, non-finalized HashContext",
    );
    assert_thrown(
        t.call("hash_update_file", vec![arg(), s("file")]),
        "TypeError",
        "hash_update_file(): Argument #1 ($context) must be a valid, non-finalized HashContext",
    );
}

/// gh12186_2.phpt
#[test]
fn cloning_a_finalized_context_fails() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "sha1");
    finish(&mut t, &ctx);
    let clone = ctx.class().handlers.clone_obj.expect("clone handler");
    assert_thrown(
        clone(&mut t.vm, &ctx).map(Value::Object),
        "ValueError",
        "Cannot clone a finalized HashContext",
    );
}

/// hash_copy_001.phpt and hash-clone.phpt, for every algorithm.
#[test]
fn hash_copy_and_clone_for_every_algorithm() {
    let mut t = TestVm::new();
    for algo in php_hash::hash_algos() {
        let orig = init(&mut t, algo);
        update(&mut t, &orig, "I can't remember anything");
        let copy = obj(ok(&mut t, "hash_copy", vec![Value::Object(orig.clone())]));
        let clone_fn = orig.class().handlers.clone_obj.unwrap();
        let cloned = clone_fn(&mut t.vm, &orig).unwrap();

        let expected = hash_hex(&mut t, algo, "I can't remember anything");
        assert_eq!(finish(&mut t, &orig), expected, "{algo}");
        update(&mut t, &copy, "Can’t tell if this is true or dream");
        update(&mut t, &cloned, "Can’t tell if this is true or dream");
        let both = hash_hex(
            &mut t,
            algo,
            "I can't remember anythingCan’t tell if this is true or dream",
        );
        assert_eq!(finish(&mut t, &copy), both, "{algo}");
        assert_eq!(finish(&mut t, &cloned), both, "{algo}");
    }
}

/// bug52240.phpt: copies keep the HMAC key.
#[test]
fn hash_copy_keeps_the_hmac_key() {
    let mut t = TestVm::new();
    let h = obj(ok(
        &mut t,
        "hash_init",
        vec![s("md5"), Value::Int(1), s("123456")],
    ));
    let h2 = obj(ok(&mut t, "hash_copy", vec![Value::Object(h.clone())]));
    assert_eq!(finish(&mut t, &h), "cab1380ea86d8acc9aa62390a58406aa");
    let h3 = obj(ok(&mut t, "hash_copy", vec![Value::Object(h2.clone())]));
    assert_eq!(finish(&mut t, &h2), "cab1380ea86d8acc9aa62390a58406aa");
    assert_eq!(finish(&mut t, &h3), "cab1380ea86d8acc9aa62390a58406aa");
}

#[test]
fn incremental_hmac_matches_hash_hmac() {
    let mut t = TestVm::new();
    for algo in php_hash::hash_hmac_algos() {
        let ctx = obj(ok(
            &mut t,
            "hash_init",
            vec![s(algo), Value::Int(1), s("secret")],
        ));
        update(&mut t, &ctx, &SAMPLE[..10]);
        update(&mut t, &ctx, &SAMPLE[10..]);
        let expected = text(&ok(
            &mut t,
            "hash_hmac",
            vec![s(algo), s(SAMPLE), s("secret")],
        ));
        assert_eq!(finish(&mut t, &ctx), expected, "{algo}");
    }
}

/// A key longer than the block size is hashed first.
#[test]
fn hmac_with_a_long_key() {
    let mut t = TestVm::new();
    let key = "k".repeat(200);
    let ctx = obj(ok(
        &mut t,
        "hash_init",
        vec![s("sha256"), Value::Int(1), s(&key)],
    ));
    update(&mut t, &ctx, "data");
    let expected = text(&ok(
        &mut t,
        "hash_hmac",
        vec![s("sha256"), s("data"), s(&key)],
    ));
    assert_eq!(finish(&mut t, &ctx), expected);
}

// ---------------------------------------------------------------------------
// HashContext class
// ---------------------------------------------------------------------------

#[test]
fn hash_context_class_shape() {
    let t = TestVm::new();
    let ce = t.vm.find_class(b"hashcontext").expect("registered");
    assert_eq!(ce.name.as_bytes(), b"HashContext");
    assert!(ce.has_flag(php_runtime::class::class_flags::FINAL));
    assert!(ce.is_internal());
    for m in ["__construct", "__serialize", "__unserialize", "__debuginfo"] {
        assert!(ce.find_method(m.as_bytes()).is_some(), "{m}");
    }
    let ctor = ce.find_method(b"__construct").unwrap();
    assert_eq!(
        ctor.flags & php_runtime::class::member_flags::PPP_MASK,
        php_runtime::class::member_flags::PRIVATE
    );
    assert!(ce.magic.debug_info.is_some());
    assert!(ce.magic.serialize.is_some() && ce.magic.unserialize.is_some());
}

/// HashContext_debugInfo.phpt
#[test]
fn hash_context_debug_info() {
    let mut t = TestVm::new();
    for algo in ["sha256", "sha3-512"] {
        let ctx = init(&mut t, algo);
        let info = t.vm.call_method(&ctx, b"__debuginfo", vec![]).unwrap();
        let info = array_of(&info);
        assert_eq!(info.len(), 1);
        assert_eq!(text(info.get_str(b"algo").unwrap()), algo);
    }
    // `snefru256` shares its implementation (and name) with `snefru`.
    let ctx = init(&mut t, "snefru256");
    let info = t.vm.call_method(&ctx, b"__debuginfo", vec![]).unwrap();
    assert_eq!(text(array_of(&info).get_str(b"algo").unwrap()), "snefru");
}

/// new-context.phpt: the private constructor refuses to run.
#[test]
fn hash_context_constructor_is_unreachable() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "md5");
    assert_thrown(
        t.vm.call_method(&ctx, b"__construct", vec![]),
        "Error",
        "Illegal call to private/final constructor",
    );
}

// ---------------------------------------------------------------------------
// HashContext::__serialize() / __unserialize()
// ---------------------------------------------------------------------------

fn not_serializable(algo: &str) -> bool {
    matches!(algo, "xxh3" | "xxh128")
}

/// hash_serialize_001.phpt: serialization round trips for every algorithm.
#[test]
fn serialization_round_trips() {
    let mut t = TestVm::new();
    for algo in php_hash::hash_algos() {
        if not_serializable(algo) {
            continue;
        }
        let expected = hash_hex(&mut t, algo, "I can't remember anything");

        let ctx0 = init(&mut t, algo);
        let ser = serialize(&mut t, &ctx0).unwrap();
        let ctx1 = unserialize(&mut t, &ser).unwrap();
        update(&mut t, &ctx1, "I can't remember anything");
        let ser = serialize(&mut t, &ctx1).unwrap();
        assert_eq!(finish(&mut t, &ctx1), expected, "{algo}");
        let ctx2 = unserialize(&mut t, &ser).unwrap();
        assert_eq!(finish(&mut t, &ctx2), expected, "{algo}");

        // Serialize in the middle of the input.
        let orig = init(&mut t, algo);
        update(&mut t, &orig, "I can't remember anything");
        let ser = serialize(&mut t, &orig).unwrap();
        let copy = unserialize(&mut t, &ser).unwrap();
        update(&mut t, &copy, "Can’t tell if this is true or dream");
        let both = hash_hex(
            &mut t,
            algo,
            "I can't remember anythingCan’t tell if this is true or dream",
        );
        assert_eq!(finish(&mut t, &copy), both, "{algo}");
    }
}

/// hash_serialize_003.phpt: the layout of an md5 context after `update("I can't remember anything")`.
#[test]
fn md5_serialization_format() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "md5");
    update(&mut t, &ctx, "I can't remember anything");
    let ser = array_of(&serialize(&mut t, &ctx).unwrap());
    assert_eq!(ser.len(), 5);
    assert_eq!(text(ser.get_int(0).unwrap()), "md5");
    assert_eq!(ser.get_int(1).unwrap().as_int(), Some(0));
    assert_eq!(ser.get_int(3).unwrap().as_int(), Some(2));
    assert_eq!(array_of(ser.get_int(4).unwrap()).len(), 0);

    let state = array_of(ser.get_int(2).unwrap());
    let ints = |range: std::ops::Range<i64>| -> Vec<i64> {
        range
            .map(|i| state.get_int(i).unwrap().as_int().unwrap())
            .collect()
    };
    assert_eq!(state.len(), 23);
    assert_eq!(
        ints(0..6),
        [25, 0, 1732584193, -271733879, -1732584194, 271733878]
    );
    let mut buffer = b"I can't remember anything".to_vec();
    buffer.resize(64, 0);
    assert_eq!(
        state.get_int(6).unwrap().as_str().unwrap().as_bytes(),
        buffer
    );
    assert!(ints(7..23).iter().all(|&n| n == 0));
}

/// hash_serialize_002.phpt
#[test]
fn hmac_contexts_cannot_be_serialized() {
    let mut t = TestVm::new();
    for algo in php_hash::hash_hmac_algos() {
        let ctx = obj(ok(
            &mut t,
            "hash_init",
            vec![s(algo), Value::Int(1), s("This is the key that I have")],
        ));
        assert_thrown(
            serialize(&mut t, &ctx),
            "Exception",
            "HashContext with HASH_HMAC option cannot be serialized",
        );
    }
}

/// bug81714.phpt and the xxh3 / xxh128 limitation
#[test]
fn unserializable_contexts() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "md5");
    finish(&mut t, &ctx);
    assert_thrown(
        serialize(&mut t, &ctx),
        "Exception",
        "HashContext for algorithm \"md5\" cannot be serialized",
    );
    for algo in ["xxh3", "xxh128"] {
        let ctx = init(&mut t, algo);
        assert_thrown(
            serialize(&mut t, &ctx),
            "Exception",
            &format!("HashContext for algorithm \"{algo}\" cannot be serialized"),
        );
        let data = Value::array(PhpArray::from_values(vec![
            s(algo),
            Value::Int(0),
            Value::empty_array(),
            Value::Int(2),
            Value::empty_array(),
        ]));
        assert_thrown(
            unserialize(&mut t, &data).map(Value::Object),
            "Exception",
            &format!("Hash algorithm \"{algo}\" cannot be unserialized"),
        );
    }
}

/// A well-formed `[algo, options, state, magic, members]` for a sha1 context, then corrupted.
fn sha1_serialization(t: &mut TestVm) -> Vec<Value> {
    let ctx = init(t, "sha1");
    update(t, &ctx, "I can't remember anything");
    let ser = array_of(&serialize(t, &ctx).unwrap());
    (0..5).map(|i| ser.get_int(i).unwrap().clone()).collect()
}

fn with(parts: &[Value], index: usize, value: Value) -> Value {
    let mut parts = parts.to_vec();
    parts[index] = value;
    Value::array(PhpArray::from_values(parts))
}

/// hash_serialize_004.phpt
#[test]
fn unserialize_errors() {
    let mut t = TestVm::new();

    // Already initialized.
    let ctx = init(&mut t, "sha256");
    let data = serialize(&mut t, &ctx).unwrap();
    assert_thrown(
        t.vm.call_method(&ctx, b"__unserialize", vec![data]),
        "Exception",
        "HashContext::__unserialize called on initialized object",
    );

    let good = sha1_serialization(&mut t);
    let ill_formed = "Incomplete or ill-formed serialization data";
    let run = |t: &mut TestVm, data: Value| unserialize(t, &data).map(Value::Object);

    // No contents; algorithm is an int; flags are a string.
    assert_thrown(run(&mut t, Value::empty_array()), "Exception", ill_formed);
    assert_thrown(
        run(&mut t, with(&good, 0, Value::Int(1))),
        "Exception",
        ill_formed,
    );
    assert_thrown(run(&mut t, with(&good, 1, s(""))), "Exception", ill_formed);
    assert_thrown(run(&mut t, with(&good, 3, s("2"))), "Exception", ill_formed);
    assert_thrown(
        run(&mut t, with(&good, 4, Value::Int(0))),
        "Exception",
        ill_formed,
    );
    // Flags indicate HASH_HMAC.
    assert_thrown(
        run(&mut t, with(&good, 1, Value::Int(1))),
        "Exception",
        "HashContext with HASH_HMAC option cannot be serialized",
    );
    // Serialization format wrong (not an array).
    assert_thrown(
        run(&mut t, with(&good, 2, s("abcdefghij"))),
        "Exception",
        &format!("{ill_formed} (\"sha1\" code -1)"),
    );
    // Serialization internals wrong: element 6 must be an int.
    let mut state = array_of(&good[2]);
    state.insert(6.into(), s(""));
    assert_thrown(
        run(&mut t, with(&good, 2, Value::array(state))),
        "Exception",
        &format!("{ill_formed} (\"sha1\" code -1024)"),
    );
    // Bad magic number.
    assert_thrown(
        run(&mut t, with(&good, 3, Value::Int(0))),
        "Exception",
        &format!("{ill_formed} (\"sha1\" code -1)"),
    );
    // Bad algorithm.
    assert_thrown(
        run(&mut t, with(&good, 0, s("xxx1"))),
        "Exception",
        "Unknown hash algorithm",
    );
    // The untouched data still loads.
    let parts = Value::array(PhpArray::from_values(good.clone()));
    assert!(run(&mut t, parts).is_ok());
}

/// xxhash_unserialize_memsize.phpt: the library's validation hooks.
#[test]
fn unserialize_validates_xxhash_state() {
    let mut t = TestVm::new();
    let ints = |v: &[i64]| {
        Value::array(PhpArray::from_values(
            v.iter().map(|&n| Value::Int(n)).collect(),
        ))
    };
    let data = |algo: &str, state: Value| {
        Value::array(PhpArray::from_values(vec![
            s(algo),
            Value::Int(0),
            state,
            Value::Int(2),
            Value::empty_array(),
        ]))
    };

    let xxh32 = ints(&[
        0,
        0,
        606290984,
        -2048144777,
        0,
        1640531535,
        0,
        0,
        0,
        0,
        30,
        0,
    ]);
    assert_thrown(
        unserialize(&mut t, &data("xxh32", xxh32)).map(Value::Object),
        "Exception",
        "Incomplete or ill-formed serialization data (\"xxh32\" code -2000)",
    );
    let xxh64 = ints(&[
        0, 0, 6, 2, 8, 9, 0, 0, 1, 5, 0, 0, 0, 0, 0, 0, 0, 0, 70, 0, 0, 0,
    ]);
    assert_thrown(
        unserialize(&mut t, &data("xxh64", xxh64)).map(Value::Object),
        "Exception",
        "Incomplete or ill-formed serialization data (\"xxh64\" code -2000)",
    );
}

#[test]
fn unserialize_loads_members_back() {
    let mut t = TestVm::new();
    let good = sha1_serialization(&mut t);
    let members = PhpArray::from_pairs([("note", s("kept"))]);
    let data = with(&good, 4, Value::array(members));
    let ctx = unserialize(&mut t, &data).unwrap();
    assert_eq!(text(&ctx.get_prop(b"note").unwrap()), "kept");
    // ... and they are serialized again.
    let again = array_of(&serialize(&mut t, &ctx).unwrap());
    assert_eq!(
        text(
            array_of(again.get_int(4).unwrap())
                .get_str(b"note")
                .unwrap()
        ),
        "kept"
    );
    // ... and cloned.
    let cloned = ctx.class().handlers.clone_obj.unwrap()(&mut t.vm, &ctx).unwrap();
    assert_eq!(text(&cloned.get_prop(b"note").unwrap()), "kept");
}

// ---------------------------------------------------------------------------
// Algorithm options (seed / secret)
// ---------------------------------------------------------------------------

const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit.";

fn hash_with(t: &mut TestVm, algo: &str, data: &str, options: Vec<(&str, Value)>) -> String {
    let r = t.vm.call_native_named(
        "hash",
        vec![s(algo), s(data)],
        vec![("options", opts(options))],
    );
    match r {
        Ok(v) => text(&v),
        Err(Flow::Exception(e)) => {
            panic!("hash() threw {}", php_runtime::Vm::exception_message(&e))
        }
        Err(other) => panic!("{other:?}"),
    }
}

fn init_with(t: &mut TestVm, algo: &str, options: Vec<(&str, Value)>) -> PResult<Value> {
    t.vm.call_native_named("hash_init", vec![s(algo)], vec![("options", opts(options))])
}

/// murmurhash3_seed.phpt
#[test]
fn murmur3_seeds() {
    let mut t = TestVm::new();
    let msg = "Two hashes meet in a bar.";
    for (algo, seed, expected) in [
        ("murmur3f", 42, "95855f9be0db784a5c37e878c4a4dcee"),
        ("murmur3c", 106, "f64c9eb40287fa686575163893e283b2"),
        ("murmur3a", 2345, "7f7ec59b"),
    ] {
        assert_eq!(
            hash_with(&mut t, algo, msg, vec![("seed", Value::Int(seed))]),
            expected
        );

        let ctx = obj(ok_named(&mut t, algo, vec![("seed", Value::Int(seed))]));
        for piece in ["Two", " hashes", " meet", " in", " a", " bar."] {
            update(&mut t, &ctx, piece);
        }
        assert_eq!(finish(&mut t, &ctx), expected);
    }
}

fn ok_named(t: &mut TestVm, algo: &str, options: Vec<(&str, Value)>) -> Value {
    match init_with(t, algo, options) {
        Ok(v) => v,
        Err(Flow::Exception(e)) => panic!(
            "hash_init() threw {}",
            php_runtime::Vm::exception_message(&e)
        ),
        Err(other) => panic!("{other:?}"),
    }
}

/// xxhash_seed.phpt
#[test]
fn xxhash_seeds() {
    let mut t = TestVm::new();
    for (algo, expected) in [
        ("xxh32", "3d0cc7e5"),
        ("xxh64", "9c9aa071b5d22a15"),
        ("xxh3", "366409913c16b70d"),
        ("xxh128", "f87856a7589354e92aeca886c71ed7fb"),
    ] {
        assert_eq!(
            hash_with(&mut t, algo, LOREM, vec![("seed", Value::Int(42))]),
            expected
        );
        let ctx = obj(ok_named(&mut t, algo, vec![("seed", Value::Int(42))]));
        update(&mut t, &ctx, "Lorem");
        update(&mut t, &ctx, " ipsum dolor");
        update(&mut t, &ctx, " sit amet,");
        update(&mut t, &ctx, " consectetur adipiscing elit.");
        assert_eq!(finish(&mut t, &ctx), expected);
    }
}

/// xxhash_options_without_seed.phpt
#[test]
fn empty_options_do_not_warn() {
    let mut t = TestVm::new();
    for algo in ["xxh32", "xxh64", "xxh3", "xxh128"] {
        let plain = hash_hex(&mut t, algo, "x");
        assert_eq!(hash_with(&mut t, algo, "x", vec![]), plain);
    }
    assert_eq!(t.output(), "");
}

/// xxhash_secret.phpt
#[test]
fn xxh3_secrets() {
    let mut t = TestVm::new();
    let secret = "a".repeat(256);
    for (algo, expected) in [
        ("xxh3", "8028aa834c03557a"),
        ("xxh128", "54279097795e7218093a05d4d781cbb9"),
    ] {
        assert_thrown(
            init_with(
                &mut t,
                algo,
                vec![("seed", Value::Int(24)), ("secret", s(&secret))],
            ),
            "Error",
            &format!("{algo}: Only one of seed or secret is to be passed for initialization"),
        );
        assert_thrown(
            init_with(&mut t, algo, vec![("secret", s(&"a".repeat(17)))]),
            "Error",
            &format!("{algo}: Secret length must be >= 136 bytes, 17 bytes passed"),
        );
        assert_eq!(
            hash_with(&mut t, algo, LOREM, vec![("secret", s(&secret))]),
            expected
        );
        let ctx = obj(ok_named(&mut t, algo, vec![("secret", s(&secret))]));
        update(&mut t, &ctx, "Lorem");
        update(
            &mut t,
            &ctx,
            " ipsum dolor sit amet, consectetur adipiscing elit.",
        );
        assert_eq!(finish(&mut t, &ctx), expected);
    }
    assert_eq!(t.output(), "");

    // Secrets beyond 256 bytes are cut, with a warning.
    let long = "a".repeat(300);
    let h = hash_with(&mut t, "xxh3", LOREM, vec![("secret", s(&long))]);
    assert_eq!(h, "8028aa834c03557a");
    assert_eq!(
        t.output(),
        "\nWarning: hash(): xxh3: Secret content exceeding 256 bytes discarded in /t.php on line 3\n"
    );
}

/// murmur_seed_deprecation.phpt, xxhash_seed_deprecation.phpt, xxh3_convert_secret_to_string.phpt
#[test]
fn option_type_deprecations() {
    let mut t = TestVm::new();
    for algo in ["murmur3a", "murmur3c", "murmur3f", "xxh32", "xxh64"] {
        ok_named(&mut t, algo, vec![("seed", s("42"))]);
    }
    let seed0 = "\nDeprecated: hash_init(): Passing a seed of a type other than int is deprecated because it is the same as setting the seed to 0 in /t.php on line 3\n";
    assert_eq!(t.output(), seed0.repeat(5));

    for algo in ["xxh3", "xxh128"] {
        ok_named(&mut t, algo, vec![("seed", s("42"))]);
    }
    let ignored = "\nDeprecated: hash_init(): Passing a seed of a type other than int is deprecated because it is ignored in /t.php on line 3\n";
    assert_eq!(t.output(), ignored.repeat(2));

    let secret = vec![("secret", Value::Int(42))];
    assert_thrown(
        init_with(&mut t, "xxh3", secret),
        "Error",
        "xxh3: Secret length must be >= 136 bytes, 2 bytes passed",
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: hash_init(): Passing a secret of a type other than string is deprecated because it implicitly converts to a string, potentially hiding bugs in /t.php on line 3\n"
    );

    // A non-int seed behaves as seed 0.
    let zero = hash_hex(&mut t, "murmur3a", "abc");
    assert_eq!(
        hash_with(&mut t, "murmur3a", "abc", vec![("seed", s("42"))]),
        zero
    );
}

#[test]
fn options_are_ignored_by_other_algorithms() {
    let mut t = TestVm::new();
    let plain = hash_hex(&mut t, "md5", "abc");
    assert_eq!(
        hash_with(&mut t, "md5", "abc", vec![("seed", Value::Int(7))]),
        plain
    );
    assert_eq!(t.output(), "");
}

// ---------------------------------------------------------------------------
// hash_pbkdf2(), hash_hkdf()
// ---------------------------------------------------------------------------

fn pbkdf2(t: &mut TestVm, args: Vec<Value>) -> Value {
    ok(t, "hash_pbkdf2", args)
}

/// hash_pbkdf2_basic.phpt
#[test]
fn pbkdf2_vectors() {
    let mut t = TestVm::new();
    let (pw, salt) = (
        "passwordPASSWORDpassword",
        "saltSALTsaltSALTsaltSALTsaltSALTsalt",
    );
    let args = |algo: &str, p: &str, sl: &str, n: i64, len: Option<i64>, raw: bool| {
        let mut v = vec![s(algo), s(p), s(sl), Value::Int(n)];
        if let Some(len) = len {
            v.push(Value::Int(len));
            v.push(Value::Bool(raw));
        }
        v
    };

    let r = pbkdf2(&mut t, args("sha1", "password", "salt", 1, Some(20), false));
    assert_eq!(text(&r), "0c60c80f961f0e71f3a9");
    let r = pbkdf2(&mut t, args("sha1", "password", "salt", 1, None, false));
    assert_eq!(text(&r), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
    let r = pbkdf2(&mut t, args("sha1", "password", "salt", 1, Some(20), true));
    assert_eq!(hex(&r), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
    let r = pbkdf2(&mut t, args("sha1", pw, salt, 4096, Some(25), false));
    assert_eq!(text(&r), "3d2eec4fe41c849b80c8d8366");
    let r = pbkdf2(&mut t, args("sha1", pw, salt, 4096, Some(25), true));
    assert_eq!(
        hex(&r),
        "3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038"
    );
    let r = pbkdf2(
        &mut t,
        args("sha256", "password", "salt", 1, Some(20), false),
    );
    assert_eq!(text(&r), "120fb6cffcf8b32c43e7");
    let r = pbkdf2(&mut t, args("sha256", "password", "salt", 1, None, false));
    assert_eq!(
        text(&r),
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
    );
    let r = pbkdf2(&mut t, args("sha256", pw, salt, 4096, Some(40), false));
    assert_eq!(text(&r), "348c89dbcbd32b2f32d814b8116e84cf2b17347e");
    let r = pbkdf2(&mut t, args("sha256", pw, salt, 4096, Some(40), true));
    assert_eq!(
        hex(&r),
        "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9"
    );
}

/// bug64745.phpt: the default length keeps the whole digest in hex mode.
#[test]
fn pbkdf2_default_length() {
    let mut t = TestVm::new();
    let hexed = pbkdf2(
        &mut t,
        vec![
            s("sha1"),
            s("password"),
            s("salt"),
            Value::Int(1),
            Value::Int(0),
        ],
    );
    let raw = pbkdf2(
        &mut t,
        vec![
            s("sha1"),
            s("password"),
            s("salt"),
            Value::Int(1),
            Value::Int(0),
            Value::Bool(true),
        ],
    );
    assert_eq!(text(&hexed), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
    assert_eq!(hex(&raw), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
}

/// hash_pbkdf2_large_length.phpt (a smaller instance of the same shape)
#[test]
fn pbkdf2_multi_block_output() {
    let mut t = TestVm::new();
    // Odd, so the last hexit is the high nibble of the first byte of the 33rd md5 block.
    let length = 1025i64;
    let r = pbkdf2(
        &mut t,
        vec![
            s("md5"),
            s("password"),
            s("salt"),
            Value::Int(1),
            Value::Int(length),
        ],
    );
    let key = text(&r);
    assert_eq!(key.len(), 1025);
    let block = ((length + 1) / 2 - 1) / 16 + 1;
    assert_eq!(block, 33);
    let mut salted = b"salt".to_vec();
    salted.extend_from_slice(&(block as u32).to_be_bytes());
    let expected = ok(
        &mut t,
        "hash_hmac",
        vec![
            s("md5"),
            Value::bytes(&salted),
            s("password"),
            Value::Bool(true),
        ],
    );
    assert_eq!(&key[1024..], &hex(&expected)[..1]);
}

/// hash_pbkdf2_error.phpt
#[test]
fn pbkdf2_errors() {
    let mut t = TestVm::new();
    for algo in ["foo", "crc32"] {
        assert_thrown(
            t.call(
                "hash_pbkdf2",
                vec![s(algo), s("password"), s("salt"), Value::Int(1)],
            ),
            "ValueError",
            "hash_pbkdf2(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm",
        );
    }
    for iterations in [0, -1] {
        assert_thrown(
            t.call(
                "hash_pbkdf2",
                vec![s("md5"), s("password"), s("salt"), Value::Int(iterations)],
            ),
            "ValueError",
            "hash_pbkdf2(): Argument #4 ($iterations) must be greater than 0",
        );
    }
    assert_thrown(
        t.call(
            "hash_pbkdf2",
            vec![
                s("md5"),
                s("password"),
                s("salt"),
                Value::Int(1),
                Value::Int(-1),
            ],
        ),
        "ValueError",
        "hash_pbkdf2(): Argument #5 ($length) must be greater than or equal to 0",
    );
}

/// hash_pbkdf2_max_length.phpt: an impossible output length exhausts memory instead of aborting.
#[test]
fn pbkdf2_huge_length_is_a_memory_error() {
    let mut t = TestVm::new();
    for raw in [false, true] {
        let r = t.call(
            "hash_pbkdf2",
            vec![
                s("md5"),
                s("password"),
                s("salt"),
                Value::Int(1),
                Value::Int(i64::MAX),
                Value::Bool(raw),
            ],
        );
        assert!(matches!(r, Err(Flow::Fatal)), "{r:?}");
        let out = t.output();
        assert!(
            out.contains(
                "Fatal error: Allowed memory size of 134217728 bytes exhausted (tried to allocate "
            ),
            "{out}"
        );
    }
}

fn hkdf(t: &mut TestVm, args: Vec<Value>) -> String {
    hex(&ok(t, "hash_hkdf", args))
}

/// hash_hkdf_rfc5869.phpt
#[test]
fn hkdf_rfc5869() {
    let mut t = TestVm::new();
    let ikm = Value::bytes(&[0x0b; 22]);
    let salt: Vec<u8> = (0..=0x0c).collect();
    let info: Vec<u8> = (0xf0..=0xf9).collect();
    let okm = hkdf(
        &mut t,
        vec![
            s("sha256"),
            ikm.clone(),
            Value::Int(42),
            Value::bytes(&info),
            Value::bytes(&salt),
        ],
    );
    assert_eq!(
        okm,
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
    );
    let okm = hkdf(&mut t, vec![s("sha256"), ikm, Value::Int(42), s(""), s("")]);
    assert_eq!(
        okm,
        "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
    );
}

/// hash_hkdf_edges.phpt
#[test]
fn hkdf_edges() {
    let mut t = TestVm::new();
    let ikm = s("input key material");
    assert_eq!(
        hkdf(&mut t, vec![s("md5"), ikm.clone(), Value::Int(7)]),
        "98b16391063ece"
    );
    assert_eq!(
        hkdf(&mut t, vec![s("md5"), ikm.clone(), Value::Int(17)]),
        "98b16391063ecee006a3ca8ee5776b1e5f"
    );
    // The default length is the digest size; names are case-insensitive.
    assert_eq!(hkdf(&mut t, vec![s("Md5"), ikm.clone()]).len(), 32);
    assert_eq!(
        hkdf(&mut t, vec![s("Md5"), ikm.clone(), Value::Int(7)]),
        "98b16391063ece"
    );
    assert_thrown(
        t.call("hash_hkdf", vec![s("jOaAt"), ikm]),
        "ValueError",
        "hash_hkdf(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm",
    );
}

/// hash_hkdf_error.phpt
#[test]
fn hkdf_errors() {
    let mut t = TestVm::new();
    let ikm = s("input key material");
    for algo in [
        "foo", "adler32", "crc32", "crc32b", "fnv132", "fnv1a32", "fnv164", "fnv1a64", "joaat",
    ] {
        assert_thrown(
            t.call("hash_hkdf", vec![s(algo), ikm.clone()]),
            "ValueError",
            "hash_hkdf(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm",
        );
    }
    assert_thrown(
        t.call("hash_hkdf", vec![s("sha1"), s("")]),
        "ValueError",
        "hash_hkdf(): Argument #2 ($key) must not be empty",
    );
    assert_thrown(
        t.call("hash_hkdf", vec![s("sha1"), ikm.clone(), Value::Int(-1)]),
        "ValueError",
        "hash_hkdf(): Argument #3 ($length) must be greater than or equal to 0",
    );
    assert_thrown(
        t.call("hash_hkdf", vec![s("sha1"), ikm, Value::Int(20 * 255 + 1)]),
        "ValueError",
        "hash_hkdf(): Argument #3 ($length) must be less than or equal to 5100",
    );
}

// ---------------------------------------------------------------------------
// hash_equals()
// ---------------------------------------------------------------------------

/// hash_equals.phpt
#[test]
fn hash_equals_compares_strings_only() {
    let mut t = TestVm::new();
    let eq = |t: &mut TestVm, a: &str, b: &str| {
        matches!(ok(t, "hash_equals", vec![s(a), s(b)]), Value::Bool(true))
    };
    assert!(eq(&mut t, "same", "same"));
    assert!(!eq(&mut t, "not1same", "not2same"));
    assert!(!eq(&mut t, "short", "longer"));
    assert!(!eq(&mut t, "longer", "short"));
    assert!(!eq(&mut t, "", "notempty"));
    assert!(!eq(&mut t, "notempty", ""));
    assert!(eq(&mut t, "", ""));

    let err = |t: &mut TestVm, a: Value, b: Value, n: usize, ty: &str| {
        assert_thrown(
            t.call("hash_equals", vec![a, b]),
            "TypeError",
            &format!(
                "hash_equals(): Argument #{n} (${}) must be of type string, {ty} given",
                if n == 1 {
                    "known_string"
                } else {
                    "user_string"
                }
            ),
        );
    };
    err(&mut t, Value::Int(123), s("NaN"), 1, "int");
    err(&mut t, s("NaN"), Value::Int(123), 2, "int");
    err(&mut t, Value::Int(123), Value::Int(123), 1, "int");
    err(&mut t, Value::Null, s(""), 1, "null");
    err(&mut t, Value::Null, Value::Int(123), 1, "null");
    err(&mut t, Value::Null, Value::Null, 1, "null");
    // sensitive_parameter.phpt
    err(&mut t, s("foo"), Value::Null, 2, "null");
    err(&mut t, Value::Bool(true), s("x"), 1, "true");
    // No deprecation about passing null.
    assert_eq!(t.output(), "");
}

// ---------------------------------------------------------------------------
// mhash
// ---------------------------------------------------------------------------

fn constant(t: &TestVm, name: &str) -> i64 {
    t.vm.constant(name)
        .and_then(|v| v.as_int())
        .unwrap_or_else(|| panic!("constant {name}"))
}

#[test]
fn constants() {
    let t = TestVm::new();
    assert_eq!(constant(&t, "HASH_HMAC"), 1);
    // gh16711_1.phpt: values and the gaps (4, 6 and 26 are unsupported).
    for (name, value) in [
        ("MHASH_CRC32", 0),
        ("MHASH_MD5", 1),
        ("MHASH_SHA1", 2),
        ("MHASH_HAVAL256", 3),
        ("MHASH_RIPEMD160", 5),
        ("MHASH_TIGER", 7),
        ("MHASH_GOST", 8),
        ("MHASH_CRC32B", 9),
        ("MHASH_SNEFRU256", 27),
        ("MHASH_MD2", 28),
        ("MHASH_XXH3", 40),
        ("MHASH_XXH128", 41),
    ] {
        assert_eq!(constant(&t, name), value, "{name}");
    }
    assert!(t.vm.constant("MHASH_SNEFRU128").is_none());
    let mhash_names = [
        "CRC32",
        "MD5",
        "SHA1",
        "HAVAL256",
        "RIPEMD160",
        "TIGER",
        "GOST",
        "CRC32B",
        "HAVAL224",
        "HAVAL192",
        "HAVAL160",
        "HAVAL128",
        "TIGER128",
        "TIGER160",
        "MD4",
        "SHA256",
        "ADLER32",
        "SHA224",
        "SHA512",
        "SHA384",
        "WHIRLPOOL",
        "RIPEMD128",
        "RIPEMD256",
        "RIPEMD320",
        "SNEFRU256",
        "MD2",
        "FNV132",
        "FNV1A32",
        "FNV164",
        "FNV1A64",
        "JOAAT",
        "CRC32C",
        "MURMUR3A",
        "MURMUR3C",
        "MURMUR3F",
        "XXH32",
        "XXH64",
        "XXH3",
        "XXH128",
    ];
    assert_eq!(mhash_names.len(), 39);
    for name in mhash_names {
        assert!(
            t.vm.constant(format!("MHASH_{name}")).is_some(),
            "MHASH_{name}"
        );
    }
}

#[test]
fn mhash_functions_are_deprecated() {
    let t = TestVm::new();
    for name in [
        "mhash",
        "mhash_count",
        "mhash_get_hash_name",
        "mhash_get_block_size",
        "mhash_keygen_s2k",
    ] {
        let f = t.vm.find_function(name).expect(name);
        assert!(f.has_flag(fn_flags::DEPRECATED), "{name}");
    }
    assert!(
        !t.vm
            .find_function("hash")
            .unwrap()
            .has_flag(fn_flags::DEPRECATED)
    );
}

/// mhash_001.phpt
#[test]
fn mhash_vectors() {
    let mut t = TestVm::new();
    let data = "This is the test of the mhash extension...";
    for (name, expected) in [
        ("MHASH_MD5", "2d9bdb91f94e96d9c4e2ae532acc936a"),
        ("MHASH_SHA1", "2f9341e55a9083edf5497bf83ba3db812a7de0a3"),
        (
            "MHASH_HAVAL256",
            "b255feff01ad641b27358dc7909bc695a1fca53bddfdfaf19020b275928793af",
        ),
        (
            "MHASH_GOST",
            "101b0a2552cebdf5137cadf15147f21e55b6432935bb9c2c03c7e28d188b2d9e",
        ),
        (
            "MHASH_TIGER",
            "953ac3799a01b9fdeb91aeab97207e67395cbb54300be00d",
        ),
        ("MHASH_CRC32", "83041db8"),
        ("MHASH_CRC32B", "df5ab7a4"),
    ] {
        let algo = constant(&t, name);
        let r = ok(&mut t, "mhash", vec![Value::Int(algo), s(data)]);
        assert_eq!(hex(&r), expected, "{name}");
    }
}

#[test]
fn mhash_with_a_key_is_an_hmac() {
    let mut t = TestVm::new();
    let r = ok(&mut t, "mhash", vec![Value::Int(1), s(SAMPLE), s("secret")]);
    assert_eq!(hex(&r), "2a632783e2812cf23de100d7d6a463ae");
    // CRC32 is not cryptographic.
    assert_thrown(
        t.call("mhash", vec![Value::Int(0), s("x"), s("key")]),
        "ValueError",
        "mhash(): Argument #1 ($algo) must be a valid cryptographic hashing algorithm",
    );
}

/// gh16711_1.phpt / gh16711_2.phpt: unknown or unsupported algorithms give false.
#[test]
fn mhash_unsupported_algorithms() {
    let mut t = TestVm::new();
    for algo in [133, 4, 6, 26, 42, -1] {
        let r = ok(
            &mut t,
            "mhash",
            vec![Value::Int(algo), Value::Int(1086849124), Value::Int(133)],
        );
        assert!(matches!(r, Value::Bool(false)), "{algo}");
        let r = ok(&mut t, "mhash_get_hash_name", vec![Value::Int(algo)]);
        assert!(matches!(r, Value::Bool(false)), "{algo}");
        let r = ok(&mut t, "mhash_get_block_size", vec![Value::Int(algo)]);
        assert!(matches!(r, Value::Bool(false)), "{algo}");
        let r = ok(
            &mut t,
            "mhash_keygen_s2k",
            vec![Value::Int(algo), s("p"), s("s"), Value::Int(8)],
        );
        assert!(matches!(r, Value::Bool(false)), "{algo}");
    }
}

/// mhash_002.phpt
#[test]
fn mhash_names_and_block_sizes() {
    let mut t = TestVm::new();
    let count = ok(&mut t, "mhash_count", vec![]).as_int().unwrap();
    assert_eq!(count, 41);
    let mut known = std::collections::HashMap::new();
    for i in 0..=count {
        if let Value::String(name) = ok(&mut t, "mhash_get_hash_name", vec![Value::Int(i)]) {
            known.insert(name.to_string(), i);
        }
    }
    for (name, len) in [
        ("MD5", 16),
        ("MD4", 16),
        ("SHA1", 20),
        ("SHA256", 32),
        ("HAVAL256", 32),
        ("HAVAL192", 24),
        ("HAVAL224", 28),
        ("HAVAL160", 20),
        ("HAVAL128", 16),
        ("RIPEMD160", 20),
        ("GOST", 32),
        ("TIGER", 24),
        ("TIGER160", 20),
        ("TIGER128", 16),
        ("CRC32", 4),
        ("CRC32B", 4),
        ("ADLER32", 4),
    ] {
        let algo = known[name];
        let r = ok(&mut t, "mhash_get_block_size", vec![Value::Int(algo)]);
        assert_eq!(r.as_int(), Some(len), "{name}");
    }
    assert!(!known.contains_key("NA_XYZ"));
    assert_eq!(known.len(), 39);
}

/// mhash_003.phpt
#[test]
fn mhash_keygen_s2k_vectors() {
    let mut t = TestVm::new();
    for (name, expected) in [
        (
            "MHASH_MD5",
            "8690154eaf9432cde9347aa15094b9c046eb06e6a0940c5479aa7a6367ae68b5e0e0745e5709fede2d9fe9739d9aad413759faa73acced821077b4ddb2788064e371eb53b3a9d55ed2839aab2655c82cfedbe83a208461c799d9d77ae481061c81539b01",
        ),
        (
            "MHASH_SHA1",
            "dd315c70061d07455d53c2fb0b08df0c61aa665c1ab1a701fa10955423248ba832a5ade406b39b78630aba3d1688e622494a0eae279d4ece9ad4bdf76e878fcb084a33c9153c2b48131d30a75b00a7c05b91f1ffeabf59bb1271c4d8a11990b84baf6d49",
        ),
    ] {
        let algo = constant(&t, name);
        let password = name.repeat(10);
        let salt = name.repeat(2);
        let r = ok(
            &mut t,
            "mhash_keygen_s2k",
            vec![Value::Int(algo), s(&password), s(&salt), Value::Int(100)],
        );
        assert_eq!(hex(&r), expected, "{name}");
    }
}

/// mhash_sensitive_parameter.phpt and gh23950.phpt
#[test]
fn mhash_keygen_s2k_length_errors() {
    let mut t = TestVm::new();
    assert_thrown(
        t.call(
            "mhash_keygen_s2k",
            vec![
                Value::Int(0),
                s("secret-password"),
                s("salt"),
                Value::Int(0),
            ],
        ),
        "ValueError",
        "mhash_keygen_s2k(): Argument #4 ($length) must be a greater than 0",
    );
    for length in [1i64 << 31, 1 << 32, (1 << 32) + 16, i64::MAX] {
        assert_thrown(
            t.call(
                "mhash_keygen_s2k",
                vec![Value::Int(1), s("password"), s("salt"), Value::Int(length)],
            ),
            "ValueError",
            "mhash_keygen_s2k(): Argument #4 ($length) must be less than or equal to 2147483647",
        );
    }
}

#[test]
fn mhash_keygen_s2k_pads_and_truncates_the_salt() {
    let mut t = TestVm::new();
    let key = |t: &mut TestVm, salt: &[u8]| {
        hex(&ok(
            t,
            "mhash_keygen_s2k",
            vec![Value::Int(1), s("pw"), Value::bytes(salt), Value::Int(16)],
        ))
    };
    // A salt is cut to 8 bytes and zero padded up to 8 bytes.
    assert_eq!(key(&mut t, b"12345678"), key(&mut t, b"12345678-ignored"));
    assert_eq!(key(&mut t, b"abc"), key(&mut t, b"abc\0\0\0\0\0"));
    assert_ne!(key(&mut t, b"abc"), key(&mut t, b"abd"));
    // The first round is plain md5(salt . password).
    let expected = hash_hex(&mut t, "md5", "abc\0\0\0\0\0pw");
    assert_eq!(key(&mut t, b"abc"), expected);
}

// ---------------------------------------------------------------------------
// Stream based variants (need the stream layer)
// ---------------------------------------------------------------------------

#[test]
fn hash_update_stream_argument_checks() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "md5");
    assert_thrown(
        t.call(
            "hash_update_stream",
            vec![Value::Object(ctx.clone()), s("not a stream")],
        ),
        "TypeError",
        "hash_update_stream(): Argument #2 ($stream) must be of type resource, string given",
    );
    // hash_update_stream_basic_001.phpt: a length of 0 reads nothing at all.
    let fake_stream = Value::Resource(ResRef::new("stream", Box::new(())));
    let r = ok(
        &mut t,
        "hash_update_stream",
        vec![Value::Object(ctx.clone()), fake_stream, Value::Int(0)],
    );
    assert_eq!(r.as_int(), Some(0));
    assert_eq!(finish(&mut t, &ctx), "d41d8cd98f00b204e9800998ecf8427e");
}

#[test]
fn hash_update_file_argument_checks() {
    let mut t = TestVm::new();
    let ctx = init(&mut t, "md5");
    assert_thrown(
        t.call(
            "hash_update_file",
            vec![Value::Object(ctx.clone()), Value::bytes(b"a\0b")],
        ),
        "ValueError",
        "hash_update_file(): Argument #2 ($filename) must not contain any null bytes",
    );
    assert_thrown(
        t.call(
            "hash_update_file",
            vec![Value::Object(ctx), s("file"), s("context")],
        ),
        "TypeError",
        "hash_update_file(): Argument #3 ($stream_context) must be of type resource or null, string given",
    );
}

fn temp_file(name: &str, contents: &str) -> String {
    let path = std::env::temp_dir().join(format!("php_in_rust_hash_{name}"));
    std::fs::write(&path, contents).unwrap();
    path.to_str().unwrap().to_owned()
}

/// hash_file_basic.phpt, hash_file_error.phpt
#[test]
#[ignore = "needs the stream layer (crate::standard::streams::read_all)"]
fn hash_file_of_real_files() {
    let mut t = TestVm::new();
    let path = temp_file(
        "hash_file.txt",
        "The quick brown fox jumped over the lazy dog.",
    );
    let r = ok(&mut t, "hash_file", vec![s("md5"), s(&path)]);
    assert_eq!(text(&r), "5c6ffbdd40d9556b73a21e63c3e0e904");
    let r = ok(
        &mut t,
        "hash_file",
        vec![s("crc32b"), s(&path), Value::Bool(true)],
    );
    assert_eq!(hex(&r), "82a34642");
    let r = ok(
        &mut t,
        "hash_file",
        vec![s("md5"), s("/nonexistent/hash_file.txt")],
    );
    assert!(matches!(r, Value::Bool(false)));
    assert!(
        t.output()
            .contains("hash_file(): Failed to open stream: No such file or directory")
    );
    let _ = std::fs::remove_file(path);
}

/// hash_hmac_file_basic.phpt
#[test]
#[ignore = "needs the stream layer (crate::standard::streams::read_all)"]
fn hash_hmac_file_of_real_files() {
    let mut t = TestVm::new();
    let path = temp_file("hash_hmac_file.txt", SAMPLE);
    let r = ok(
        &mut t,
        "hash_hmac_file",
        vec![s("md5"), s(&path), s("secret")],
    );
    assert_eq!(text(&r), "2a632783e2812cf23de100d7d6a463ae");
    let _ = std::fs::remove_file(path);
}

/// hash_update_file.phpt
#[test]
#[ignore = "needs the stream layer (crate::standard::streams::read_all)"]
fn hash_update_file_of_real_files() {
    let mut t = TestVm::new();
    let path = temp_file(
        "hash_update_file.txt",
        "The quick brown fox jumped over the lazy dog.",
    );
    let ctx = init(&mut t, "md5");
    let r = ok(
        &mut t,
        "hash_update_file",
        vec![Value::Object(ctx.clone()), s(&path)],
    );
    assert!(matches!(r, Value::Bool(true)));
    assert_eq!(finish(&mut t, &ctx), "5c6ffbdd40d9556b73a21e63c3e0e904");
    let ctx = init(&mut t, "md5");
    let r = ok(
        &mut t,
        "hash_update_file",
        vec![Value::Object(ctx), s("/nonexistent/x")],
    );
    assert!(matches!(r, Value::Bool(false)));
    let _ = std::fs::remove_file(path);
}
