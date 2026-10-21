//! HMAC / PBKDF2 / HKDF / registry checks against `ext/hash/tests/*.phpt`.

use php_hash::{
    HashAlgorithm, HashContext, HashError, HashOptions, Hmac, algorithms, find, hash, hash_algos,
    hash_hmac_algos, hkdf, hmac, md5, md5_hex, pbkdf2, sha1_hex, to_hex,
};

/// `hash_hmac_basic.phpt`: HMAC of a sample string with the key "secret".
const HMAC_BASIC: &[(&str, &str)] = &[
    (
        "gost",
        "a4a3c80bdf3f8665bf07376a34dc9c1b11af7c813f4928f62e39f0c0dc564dad",
    ),
    ("haval128,3", "4d1318607f0406bd1b7bd50907772672"),
    ("md2", "6d111dab563025e4cb5f4425c991fa12"),
    ("md4", "10cdbfe843000c623f8b8da0d5d20b0b"),
    ("md5", "2a632783e2812cf23de100d7d6a463ae"),
    ("ripemd128", "26c2f694a65b1928b668cf55f65529b4"),
    ("ripemd160", "4b3433ba596ec39692bb7ce760a9ee5fb818113f"),
    (
        "ripemd256",
        "4e4e5ec19322895a727f272dfe68f87bc1af66cc6ce27c6c1360a5ee78a14b30",
    ),
    (
        "ripemd320",
        "f10a8ff82e828b92a5ff0a02fc9032bc61352d0d824821fc42f7e09cf5b5f41ee59fd33a730d7469",
    ),
    ("sha1", "5bfdb62b97e2c987405463e9f7c193139c0e1fd0"),
    (
        "sha256",
        "49bde3496b9510a17d0edd8a4b0ac70148e32a1d51e881ec76faa96534125838",
    ),
    (
        "sha384",
        "b781415b856744834e532b9899e1aa0bec5a82cf09a838f0a833470468e2a42648a52428cfd9012385d04de5cd9bd122",
    ),
    (
        "sha512",
        "7de05636b18e2b0ca3427e03f53074af3a48a7b9df226daba4f22324c570638e7d7b26430e214799c9ce0db5ee88dad3292ca0f38bf99b8eaebed59b3a9c140a",
    ),
    (
        "snefru",
        "67af483046f9cf16fe19f9087929ccfc6ad176ade3290b4d33f43e0ddb07e711",
    ),
    (
        "tiger192,3",
        "00a0f884f15a9e5549ed0e40ca0190522d369027e16d5b59",
    ),
    (
        "whirlpool",
        "4a0f1582b21b7aff59bfba7f9c29131c69741b2ce80acdc7d314040f3b768cf5a17e30b74cceb86fbc6b34b1692e0addd5bfd7cfc043d40c0621f1b97e26fa49",
    ),
    ("md5(raw)", "2a632783e2812cf23de100d7d6a463ae"),
    (
        "sha256(raw)",
        "49bde3496b9510a17d0edd8a4b0ac70148e32a1d51e881ec76faa96534125838",
    ),
];

const HMAC_BASIC_CONTENT: &[u8] =
    b"This is a sample string used to test the hash_hmac function with various hashing algorithms";
const HMAC_BASIC_KEY: &[u8] = b"secret";

/// `hash_hkdf_basic.phpt`: default-length HKDF of "input key material".
const HKDF_BASIC: &[(&str, &str)] = &[
    ("md2", "87779851d2377dab25da16fd7aadfdf5"),
    ("md4", "422c6bd8dd2a6baae8abadef618c3ede"),
    ("md5", "98b16391063ecee006a3ca8ee5776b1e"),
    ("sha1", "a71863230e3782240265126a53e137af6667e988"),
    (
        "sha224",
        "51678ceb17e803505187b2cf6451c30fbc572fda165bb69bbd117c7a",
    ),
    (
        "sha256",
        "d8f0bede4b652933c32a92eccf7723f7eeb4701744c81325dc3f0fa9fda24499",
    ),
    (
        "sha384",
        "f600680e677bb417a7a22a4da8b167c0d91823a7a5d56a49aeb1838bb2320c05068d15d6d980824fee542a279d310c3a",
    ),
    (
        "sha512",
        "fb1b86549e941b81821a89ac6ba7c4f93465077b3f2af94352ebf1d041efcd3c5694469c1ae31bb10db4c1d2ab84f07e4518ba33a3eadd4a149425750285c640",
    ),
    ("ripemd128", "cb6418fc0dc9efaeb7e9654390fa7f14"),
    ("ripemd160", "ba42dbb34f08e9337ace15295f218754a41d6c39"),
    (
        "ripemd256",
        "f2e96b292935e2395b59833ed89d928ac1197ff62c8031ebc06a3f5bad19513f",
    ),
    (
        "ripemd320",
        "a13a682072525ceb4c4a5fef59096e682096e1096e6e7e238c7bd48a6f6c6a9ba3d7d9fbee6b68c4",
    ),
    (
        "whirlpool",
        "497c717e04d896c3d582742c614435b7d0963b39de12dcf532540d39164b3b85214014620dfdff4a089a06b06aff43c39a3b4d9b806913cf6309de58ff1151f5",
    ),
    ("tiger128,3", "e13c2e7262892c6bd8dfc24121e7cb34"),
    ("tiger160,3", "48cc5a9f5e5d7029eb0544662222c0ba13822b7b"),
    (
        "tiger192,3",
        "5a665d23b6cbb405668160e58b01aebef74eba979f4bc70b",
    ),
    ("tiger128,4", "8acf517ecf58cccbd65c1186d71e4116"),
    ("tiger160,4", "cc0e33ee26700a2eb9a994bbb0e6cef29b429441"),
    (
        "tiger192,4",
        "97fa02d42331321fdc05c7f8dbc756d751ca36ce1aee69b0",
    ),
    ("haval128,3", "2accab8029d42fb15fdbe9d3e2a470ca"),
    ("haval160,3", "496fd29e7fc8351d2971b96a3733a7b3de000064"),
    (
        "haval192,3",
        "238a731801439b1f195e1a1568ce75251e1dd719d904a8a2",
    ),
    (
        "haval224,3",
        "d863e596ff6b2bdba1ed7b313df1c3d177176312e81b47e9290f7566",
    ),
    (
        "haval256,3",
        "96f555fe41255c34fe57b275f1ae40bbb8f07c6a2a6d68c849748fbb393ff443",
    ),
    ("haval128,4", "9822af229cc59527a72e231a690fad3b"),
    ("haval160,4", "1bbbc4d632daaf94d5ba167efaa70af5b753effe"),
    (
        "haval192,4",
        "dd12a8f8919cbf5632497f0918b30236371dd1b55f71e824",
    ),
    (
        "haval224,4",
        "8af449fb4eb627eb8887507c1279a116ac4325b5806dd22e2f2af410",
    ),
    (
        "haval256,4",
        "bd74a6d5fa1ec23a92ce1fd76c36bc8be36f5eddbea821545a91810e1f8d6fc5",
    ),
    ("haval128,5", "84564f3450a6ccf6041162207dc8acba"),
    ("haval160,5", "b55cd1b3c514457b9e61c51ad22f302f6ec7cca1"),
    (
        "haval192,5",
        "d1db7a8e69b327455d530d1ac60f774023b8b4bdd6bbbf92",
    ),
    (
        "haval224,5",
        "c5a2576511f1143c6e29f63d82d6e0be8f67d0bea448e27238be5000",
    ),
    (
        "haval256,5",
        "9dbab73d13f1fd3a1b41398fe90ba1f298329681d861b023373c33f1051bd4d3",
    ),
    (
        "snefru",
        "798eac954e5ece38e9acb63b50c1c2ecb799d34356358cec5a80eeeea91c8de9",
    ),
    (
        "snefru256",
        "798eac954e5ece38e9acb63b50c1c2ecb799d34356358cec5a80eeeea91c8de9",
    ),
    (
        "gost",
        "64edd584b87a2dfdd1f2b44ed2db8bd27af8386aafe751c2aebaed32dfa3852e",
    ),
];

/// `hash_hkdf_rfc5869.phpt`: (algo, ikm, length, info, salt, expected).
#[allow(clippy::type_complexity)]
const HKDF_RFC5869: &[(&str, &[u8], usize, &[u8], &[u8], &str)] = &[
    ("sha256", b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", 42, b"\xf0\xf1\xf2\xf3\xf4\xf5\xf6\xf7\xf8\xf9", b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c", "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"),
    ("sha256", b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c\r\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNO", 82, b"\xb0\xb1\xb2\xb3\xb4\xb5\xb6\xb7\xb8\xb9\xba\xbb\xbc\xbd\xbe\xbf\xc0\xc1\xc2\xc3\xc4\xc5\xc6\xc7\xc8\xc9\xca\xcb\xcc\xcd\xce\xcf\xd0\xd1\xd2\xd3\xd4\xd5\xd6\xd7\xd8\xd9\xda\xdb\xdc\xdd\xde\xdf\xe0\xe1\xe2\xe3\xe4\xe5\xe6\xe7\xe8\xe9\xea\xeb\xec\xed\xee\xef\xf0\xf1\xf2\xf3\xf4\xf5\xf6\xf7\xf8\xf9\xfa\xfb\xfc\xfd\xfe\xff", b"`abcdefghijklmnopqrstuvwxyz{|}~\x7f\x80\x81\x82\x83\x84\x85\x86\x87\x88\x89\x8a\x8b\x8c\x8d\x8e\x8f\x90\x91\x92\x93\x94\x95\x96\x97\x98\x99\x9a\x9b\x9c\x9d\x9e\x9f\xa0\xa1\xa2\xa3\xa4\xa5\xa6\xa7\xa8\xa9\xaa\xab\xac\xad\xae\xaf", "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87"),
    ("sha256", b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", 42, b"", b"", "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"),
    ("sha1", b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", 42, b"\xf0\xf1\xf2\xf3\xf4\xf5\xf6\xf7\xf8\xf9", b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c", "085a01ea1b10f36933068b56efa5ad81a4f14b822f5b091568a9cdd4f155fda2c22e422478d305f3f896"),
    ("sha1", b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c\r\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNO", 82, b"\xb0\xb1\xb2\xb3\xb4\xb5\xb6\xb7\xb8\xb9\xba\xbb\xbc\xbd\xbe\xbf\xc0\xc1\xc2\xc3\xc4\xc5\xc6\xc7\xc8\xc9\xca\xcb\xcc\xcd\xce\xcf\xd0\xd1\xd2\xd3\xd4\xd5\xd6\xd7\xd8\xd9\xda\xdb\xdc\xdd\xde\xdf\xe0\xe1\xe2\xe3\xe4\xe5\xe6\xe7\xe8\xe9\xea\xeb\xec\xed\xee\xef\xf0\xf1\xf2\xf3\xf4\xf5\xf6\xf7\xf8\xf9\xfa\xfb\xfc\xfd\xfe\xff", b"`abcdefghijklmnopqrstuvwxyz{|}~\x7f\x80\x81\x82\x83\x84\x85\x86\x87\x88\x89\x8a\x8b\x8c\x8d\x8e\x8f\x90\x91\x92\x93\x94\x95\x96\x97\x98\x99\x9a\x9b\x9c\x9d\x9e\x9f\xa0\xa1\xa2\xa3\xa4\xa5\xa6\xa7\xa8\xa9\xaa\xab\xac\xad\xae\xaf", "0bd770a74d1160f7c9f12cd5912a06ebff6adcae899d92191fe4305673ba2ffe8fa3f1a4e5ad79f3f334b3b202b2173c486ea37ce3d397ed034c7f9dfeb15c5e927336d0441f4c4300e2cff0d0900b52d3b4"),
    ("sha1", b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", 42, b"", b"", "0ac1af7002b3d761d1e55298da9d0506b9ae52057220a306e07b6b87e8df21d0ea00033de03984d34918"),
    ("sha1", b"\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c\x0c", 42, b"", b"", "2c91117204d745f3500d636a62f64f0ab3bae548aa53d423b0d1f27ebba6f5e5673a081d70cce7acfc48"),
];

/// Expected `hash_algos()` (hash_algos.phpt).
const ALGOS: [&str; 60] = [
    "md2",
    "md4",
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512/224",
    "sha512/256",
    "sha512",
    "sha3-224",
    "sha3-256",
    "sha3-384",
    "sha3-512",
    "ripemd128",
    "ripemd160",
    "ripemd256",
    "ripemd320",
    "whirlpool",
    "tiger128,3",
    "tiger160,3",
    "tiger192,3",
    "tiger128,4",
    "tiger160,4",
    "tiger192,4",
    "snefru",
    "snefru256",
    "gost",
    "gost-crypto",
    "adler32",
    "crc32",
    "crc32b",
    "crc32c",
    "fnv132",
    "fnv1a32",
    "fnv164",
    "fnv1a64",
    "joaat",
    "murmur3a",
    "murmur3c",
    "murmur3f",
    "xxh32",
    "xxh64",
    "xxh3",
    "xxh128",
    "haval128,3",
    "haval160,3",
    "haval192,3",
    "haval224,3",
    "haval256,3",
    "haval128,4",
    "haval160,4",
    "haval192,4",
    "haval224,4",
    "haval256,4",
    "haval128,5",
    "haval160,5",
    "haval192,5",
    "haval224,5",
    "haval256,5",
];

const NON_CRYPTO: [&str; 16] = [
    "adler32", "crc32", "crc32b", "crc32c", "fnv132", "fnv1a32", "fnv164", "fnv1a64", "joaat",
    "murmur3a", "murmur3c", "murmur3f", "xxh32", "xxh64", "xxh3", "xxh128",
];

#[test]
fn registry_order_and_flags() {
    assert_eq!(hash_algos(), ALGOS);
    let crypto: Vec<&str> = ALGOS
        .iter()
        .copied()
        .filter(|a| !NON_CRYPTO.contains(a))
        .collect();
    assert_eq!(hash_hmac_algos(), crypto);
    assert_eq!(algorithms().len(), 60);
    for algo in algorithms() {
        assert_eq!(
            algo.is_crypto,
            !NON_CRYPTO.contains(&algo.name),
            "{}",
            algo.name
        );
        // The registry sizes agree with what the hashers actually produce.
        let digest = hash(algo.name, b"size check").unwrap();
        assert_eq!(digest.len(), algo.digest_size, "{}", algo.name);
        let h = algo.new_hasher(&HashOptions::none()).unwrap();
        assert_eq!(h.digest_size(), algo.digest_size);
        assert_eq!(h.block_size(), algo.block_size);
        assert_eq!(h.algorithm(), algo);
    }
}

#[test]
fn lookup_is_case_insensitive() {
    assert_eq!(find("SHA256").map(|a| a.name), Some("sha256"));
    assert_eq!(
        HashAlgorithm::find("Haval128,3").map(|a| a.name),
        Some("haval128,3")
    );
    assert!(find("nope").is_none());
    assert_eq!(hash("nope", b""), Err(HashError::UnknownAlgorithm));
    // snefru256 is an alias that serialises as "snefru".
    assert_eq!(find("snefru256").unwrap().serialized_name, "snefru");
    assert_eq!(find("snefru").unwrap().serialized_name, "snefru");
}

#[test]
fn hmac_basic() {
    for (name, expected) in HMAC_BASIC {
        let algo = name.trim_end_matches("(raw)");
        let mac = hmac(algo, HMAC_BASIC_KEY, HMAC_BASIC_CONTENT).unwrap();
        assert_eq!(to_hex(&mac), *expected, "{name}");
    }
}

#[test]
fn hmac_rfc2104_md5() {
    // hmac-md5.phpt, driven through the incremental API.
    let mut m = Hmac::new("md5", &[0x0b; 16]).unwrap();
    m.update(b"Hi ");
    m.update(b"There");
    assert_eq!(to_hex(&m.finalize()), "9294727a3638bb1c13f48ef8158bfc9d");

    let mut ctx = HashContext::new_hmac("md5", b"Jefe", &HashOptions::none()).unwrap();
    assert!(ctx.is_hmac());
    ctx.update(b"what do ya want for nothing?");
    assert_eq!(to_hex(&ctx.finalize()), "750c783e6ab0b503eaa86e310a5db738");

    let mac = hmac("md5", &[0xaa; 16], &[0xdd; 50]).unwrap();
    assert_eq!(to_hex(&mac), "56be34521d144c88dbb8c733f0e8b3f6");
}

#[test]
fn hmac_key_longer_than_block_and_empty_key() {
    // RFC 2202 test case 6 (80 byte key, md5 block size is 64).
    let mac = hmac(
        "md5",
        &[0xaa; 80],
        b"Test Using Larger Than Block-Size Key - Hash Key First",
    )
    .unwrap();
    assert_eq!(to_hex(&mac), "6b1ab7fe4bd7bf8f0b62e6ce61b9d0cd");
    // hash_hmac() accepts an empty key (RFC 2104: equivalent to a zero key).
    assert_eq!(
        to_hex(&hmac("sha256", b"", b"").unwrap()),
        "b613679a0814d9ec772f95d778c35fc5ff1697c493715653c6c712144292c5ad"
    );
}

#[test]
fn hmac_errors() {
    assert_eq!(hmac("crc32", b"k", b"d"), Err(HashError::NotCryptographic));
    assert_eq!(hmac("nope", b"k", b"d"), Err(HashError::NotCryptographic));
    assert!(matches!(
        HashContext::new_hmac("xxh3", b"k", &HashOptions::none()),
        Err(HashError::NotCryptographic)
    ));
    assert!(matches!(
        HashContext::new_hmac("nope", b"k", &HashOptions::none()),
        Err(HashError::UnknownAlgorithm)
    ));
    assert!(matches!(
        HashContext::new_hmac("md5", b"", &HashOptions::none()),
        Err(HashError::EmptyKey)
    ));
}

#[test]
fn hmac_context_clone_and_serialize() {
    let mut ctx = HashContext::new_hmac("sha256", b"key", &HashOptions::none()).unwrap();
    ctx.update(b"part one ");
    let mut copy = ctx.clone();
    ctx.update(b"part two");
    copy.update(b"part two");
    let expected = hmac("sha256", b"key", b"part one part two").unwrap();
    assert_eq!(ctx.clone().finalize(), expected);
    assert_eq!(copy.finalize(), expected);
    // HMAC contexts refuse serialisation; plain ones work.
    assert!(ctx.serialize_state().is_err());
    let mut plain = HashContext::new("sha256", &HashOptions::none()).unwrap();
    plain.update(b"abc");
    let (magic, state) = plain.serialize_state().unwrap();
    let mut restored = HashContext::from_state("sha256", magic, &state).unwrap();
    restored.update(b"def");
    assert_eq!(restored.finalize(), hash("sha256", b"abcdef").unwrap());
    assert!(matches!(
        HashContext::from_state("sha256", 0, &state),
        Err(HashError::InvalidState(_))
    ));
}

#[test]
fn pbkdf2_basic() {
    let run = |algo, pass: &str, salt: &str, iters, len, raw| {
        let out = pbkdf2(algo, pass.as_bytes(), salt.as_bytes(), iters, len, raw).unwrap();
        if raw {
            to_hex(&out)
        } else {
            String::from_utf8(out).unwrap()
        }
    };
    let long_pass = "passwordPASSWORDpassword";
    let long_salt = "saltSALTsaltSALTsaltSALTsaltSALTsalt";
    assert_eq!(
        run("sha1", "password", "salt", 1, 20, false),
        "0c60c80f961f0e71f3a9"
    );
    assert_eq!(
        run("sha1", "password", "salt", 1, 0, false),
        "0c60c80f961f0e71f3a9b524af6012062fe037a6"
    );
    assert_eq!(
        run("sha1", "password", "salt", 1, 20, true),
        "0c60c80f961f0e71f3a9b524af6012062fe037a6"
    );
    assert_eq!(
        run("sha1", long_pass, long_salt, 4096, 25, false),
        "3d2eec4fe41c849b80c8d8366"
    );
    assert_eq!(
        run("sha1", long_pass, long_salt, 4096, 25, true),
        "3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038"
    );
    assert_eq!(
        run("sha256", "password", "salt", 1, 20, false),
        "120fb6cffcf8b32c43e7"
    );
    assert_eq!(
        run("sha256", "password", "salt", 1, 0, false),
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
    );
    assert_eq!(
        run("sha256", "password", "salt", 1, 20, true),
        "120fb6cffcf8b32c43e7225256c4f837a86548c9"
    );
    assert_eq!(
        run("sha256", long_pass, long_salt, 4096, 40, false),
        "348c89dbcbd32b2f32d814b8116e84cf2b17347e"
    );
    assert_eq!(
        run("sha256", long_pass, long_salt, 4096, 40, true),
        "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9"
    );
}

#[test]
fn pbkdf2_block_counter_and_errors() {
    // More than 255 blocks: the big-endian block counter spills into its second byte.
    let out = pbkdf2("md5", b"password", b"salt", 1, 5000, true).unwrap();
    assert_eq!(out.len(), 5000);
    let block = 300usize; // bytes 4800..4816 belong to block 301
    let mut salted = b"salt".to_vec();
    salted.extend_from_slice(&(block as u32 + 1).to_be_bytes());
    let expected = hmac("md5", b"password", &salted).unwrap();
    assert_eq!(&out[block * 16..block * 16 + 8], &expected[..8]);
    // hex: odd lengths truncate the final digit.
    let hex = pbkdf2("sha1", b"password", b"salt", 2, 7, false).unwrap();
    assert_eq!(hex.len(), 7);
    assert_eq!(
        pbkdf2("crc32", b"p", b"s", 1, 0, false),
        Err(HashError::NotCryptographic)
    );
    assert_eq!(
        pbkdf2("sha1", b"p", b"s", 0, 0, false),
        Err(HashError::InvalidIterations)
    );
    assert!(matches!(
        pbkdf2("md5", b"p", b"s", 1, usize::MAX, true),
        Err(HashError::LengthTooLarge { .. })
    ));
}

#[test]
fn hkdf_vectors() {
    for (algo, expected) in HKDF_BASIC {
        let out = hkdf(algo, b"input key material", 0, b"", b"").unwrap();
        assert_eq!(to_hex(&out), *expected, "{algo}");
    }
    for (algo, ikm, length, info, salt, expected) in HKDF_RFC5869 {
        let out = hkdf(algo, ikm, *length, info, salt).unwrap();
        assert_eq!(to_hex(&out), *expected, "{algo} {length}");
    }
}

#[test]
fn hkdf_edges_and_errors() {
    let ikm = b"input key material";
    assert_eq!(
        to_hex(&hkdf("md5", ikm, 7, b"", b"").unwrap()),
        "98b16391063ece"
    );
    assert_eq!(
        to_hex(&hkdf("md5", ikm, 17, b"", b"").unwrap()),
        "98b16391063ecee006a3ca8ee5776b1e5f"
    );
    assert_eq!(
        to_hex(&hkdf("Md5", ikm, 7, b"", b"").unwrap()),
        "98b16391063ece"
    );
    assert_eq!(
        hkdf("jOaAt", ikm, 0, b"", b""),
        Err(HashError::NotCryptographic)
    );
    assert_eq!(hkdf("md5", b"", 0, b"", b""), Err(HashError::EmptyInput));
    assert_eq!(
        hkdf("md5", ikm, 16 * 255 + 1, b"", b""),
        Err(HashError::LengthTooLarge { max: 16 * 255 })
    );
    // The maximum length works and spans every counter value.
    assert_eq!(
        hkdf("md5", ikm, 16 * 255, b"", b"").unwrap().len(),
        16 * 255
    );
}

#[test]
fn md5_sha1_and_crc32_helpers() {
    assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex(b"string"), to_hex(&hash("md5", b"string").unwrap()));
    assert_eq!(md5(b"a").to_vec(), hash("md5", b"a").unwrap());
    assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(
        sha1_hex(b"string"),
        to_hex(&hash("sha1", b"string").unwrap())
    );
    // PHP docs: crc32("The quick brown fox jumped over the lazy dog.") == 2191738434.
    assert_eq!(
        php_hash::crc32(b"The quick brown fox jumped over the lazy dog."),
        2191738434
    );
    assert_eq!(php_hash::crc32(b""), 0);
    assert_eq!(
        to_hex(&hash("crc32b", b"The quick brown fox jumped over the lazy dog.").unwrap()),
        format!("{:08x}", 2191738434u32)
    );
}

#[test]
fn to_hex_is_lower_case() {
    assert_eq!(to_hex(&[]), "");
    assert_eq!(to_hex(&[0xde, 0xad, 0xbe, 0xef, 0x01]), "deadbeef01");
}
