//! Unit tests for iptc.c, ported from `tests/phpt/ext/standard/tests/image/iptc*.phpt`
//! and `bug67250.phpt` / `bug70096.phpt` / `gh20582.phpt`.

use std::io::Cursor;

use php_runtime::{ArrayKey, PhpRef, Value};

use crate::testing::{TestVm, thrown};

use super::embed_stream;

fn scratch(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("php_iptc_{}_{name}", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

fn bytes(v: &Value) -> Vec<u8> {
    v.as_str().expect("string").as_bytes().to_vec()
}

fn is_false(v: &Value) -> bool {
    matches!(v, Value::Bool(false))
}

fn b64_decode(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes().take_while(|&c| c != b'=') {
        acc = (acc << 6) | val(c) as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn b64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.iter().fold(0u32, |a, &b| (a << 8) | b as u32) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        for _ in chunk.len()..3 {
            out.push('=');
        }
    }
    out
}

/// `iptc_make_tag` from the iptcembed_002 test.
fn iptc_make_tag(rec: u8, data: u8, value: &str) -> Vec<u8> {
    let length = value.len();
    let mut r = vec![0x1c, rec, data];
    if length < 0x8000 {
        r.push((length >> 8) as u8);
        r.push((length & 0xff) as u8);
    } else {
        r.extend_from_slice(&[0x80, 0x04]);
        r.extend_from_slice(&(length as u32).to_be_bytes());
    }
    r.extend_from_slice(value.as_bytes());
    r
}

fn dump_parse(v: &Value) -> Vec<(String, Vec<String>)> {
    let Value::Array(a) = v else {
        panic!("not an array: {v:?}")
    };
    a.iter()
        .map(|(k, v)| {
            let ArrayKey::Str(k) = k else { panic!() };
            let Value::Array(list) = &*v.deref() else {
                panic!()
            };
            (
                k.to_string_lossy().into_owned(),
                list.values()
                    .map(|s| s.as_str().unwrap().to_string_lossy().into_owned())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn iptcparse_bug67250_out_of_bounds() {
    let mut t = TestVm::new();
    let r = t
        .call("iptcparse", vec![Value::from(b"\x1C\x02_\x80___".to_vec())])
        .unwrap();
    assert!(is_false(&r));
}

#[test]
fn iptcparse_collects_records() {
    let mut t = TestVm::new();
    let mut data = b"junk before the first tag".to_vec();
    data.extend(iptc_make_tag(2, 105, "Tauren"));
    data.extend(iptc_make_tag(2, 25, "a"));
    data.extend(iptc_make_tag(1, 90, ""));
    data.extend(iptc_make_tag(2, 25, "b"));
    let r = t
        .call("iptcparse", vec![Value::from(data.clone())])
        .unwrap();
    assert_eq!(
        dump_parse(&r),
        vec![
            ("2#105".to_string(), vec!["Tauren".to_string()]),
            ("2#025".to_string(), vec!["a".to_string(), "b".to_string()]),
            ("1#090".to_string(), vec![String::new()]),
        ]
    );
    // iptc.c refuses a tag that ends exactly at the end of the block
    // (`inx + 4 >= str_len`): a trailing empty tag is dropped.
    data.extend(iptc_make_tag(1, 91, ""));
    let r = t.call("iptcparse", vec![Value::from(data)]).unwrap();
    assert_eq!(dump_parse(&r).len(), 3);
}

#[test]
fn iptcparse_long_tags_and_garbage() {
    let mut t = TestVm::new();
    let long = "x".repeat(0x8000 + 3);
    let mut data = iptc_make_tag(2, 120, &long);
    data.extend(iptc_make_tag(2, 5, "tail"));
    let r = t.call("iptcparse", vec![Value::from(data)]).unwrap();
    let parsed = dump_parse(&r);
    assert_eq!(parsed[0].0, "2#120");
    assert_eq!(parsed[0].1[0].len(), long.len());
    assert_eq!(parsed[1], ("2#005".to_string(), vec!["tail".to_string()]));
    // Parsing stops at the first byte that is not a tag marker.
    let mut data = iptc_make_tag(2, 5, "ok");
    data.extend_from_slice(b"\x00\x1c\x02\x06\x00\x01z");
    let r = t.call("iptcparse", vec![Value::from(data)]).unwrap();
    assert_eq!(dump_parse(&r).len(), 1);
    // No tag at all, truncated tags, empty input.
    for data in [
        &b""[..],
        b"hello",
        b"\x1c\x02\x05\x00",
        b"\x1c\x01\x05\x00\x09ab",
    ] {
        let r = t
            .call("iptcparse", vec![Value::from(data.to_vec())])
            .unwrap();
        assert!(is_false(&r), "{data:?}");
    }
}

#[test]
fn iptcembed_001_wrong_file() {
    let file = scratch("001.data");
    std::fs::write(&file, "-1-1").unwrap();
    let mut t = TestVm::new();
    // iptcembed(-1, $file, -1)
    let r = t
        .call(
            "iptcembed",
            vec![Value::Int(-1), Value::from(file.clone()), Value::Int(-1)],
        )
        .unwrap();
    assert!(is_false(&r));
    std::fs::remove_file(&file).unwrap();
    assert_eq!(t.output(), "");
}

#[test]
fn iptcembed_003_unable_to_open() {
    let mut t = TestVm::new();
    let name = "/definitely/missing/iptcembed/DOES_NOT_EXIST.ERR";
    let r = t
        .call(
            "iptcembed",
            vec![Value::Int(-1), Value::str(name), Value::Int(-1)],
        )
        .unwrap();
    assert!(is_false(&r));
    assert_eq!(
        t.output(),
        format!("\nWarning: iptcembed(): Unable to open {name} in /t.php on line 3\n")
    );
}

#[test]
fn iptcembed_null_bytes_in_filename() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call(
        "iptcembed",
        vec![Value::str("A"), Value::str("/tmp/a.jpg\0x")],
    ));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "iptcembed(): Argument #2 ($filename) must not contain any null bytes"
    );
}

/// `$base64_1x1_jpeg` of iptcembed_002.phpt.
fn jpeg_1x1() -> Vec<u8> {
    let phpt = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/phpt/ext/standard/tests/image/iptcembed_002.phpt"
    ))
    .unwrap();
    let start = phpt.find("$base64_1x1_jpeg=\"").unwrap() + "$base64_1x1_jpeg=\"".len();
    let end = start + phpt[start..].find('"').unwrap();
    b64_decode(&phpt[start..end])
}

#[test]
fn iptcembed_002_valid_jpg_roundtrip() {
    let mut t = TestVm::new();
    let file = scratch("1x1.jpg");
    let file2 = scratch("1x1_with_iptc_tags.jpg");
    let jpeg = jpeg_1x1();
    std::fs::write(&file, &jpeg).unwrap();
    assert_eq!(php_hash::md5_hex(&jpeg), "07dd8594450e8c18ab8a79d7cb4573c7");

    // list($width, $height, $type, $attr) = getimagesize($file, $info);
    let info = PhpRef::new(Value::Null);
    let r = t
        .call(
            "getimagesize",
            vec![Value::from(file.clone()), Value::Ref(info.clone())],
        )
        .unwrap();
    let Value::Array(a) = &r else { panic!() };
    assert_eq!(a.get(&ArrayKey::Int(0)).unwrap().as_int(), Some(1));
    assert_eq!(a.get(&ArrayKey::Int(1)).unwrap().as_int(), Some(1));
    assert_eq!(a.get(&ArrayKey::Int(2)).unwrap().as_int(), Some(2));
    let Value::Array(i) = info.get() else {
        panic!()
    };
    assert!(i.get_str(b"APP0").is_some());

    let tags = [
        ("2#105", "Tauren"),
        ("2#120", "Tauren with Trunk"),
        ("2#110", "Copyright 2004-2016, Blizzard"),
        ("2#025", "Tauren, Chaman, Blizzard"),
        ("2#090", "Thunder Bluffs"),
    ];
    let mut iptc = Vec::new();
    for (tag, string) in tags {
        let rec = tag.as_bytes()[0] - b'0';
        let data: u8 = tag[2..].parse().unwrap();
        iptc.extend(iptc_make_tag(rec, data, string));
    }
    assert_eq!(
        php_hash::md5_hex(b64_encode(&iptc).as_bytes()),
        "7056c4b3060f92a4f9e5b7d0caa61859"
    );

    let content = t
        .call(
            "iptcembed",
            vec![Value::from(iptc), Value::from(file.clone()), Value::Int(0)],
        )
        .unwrap();
    assert!(!is_false(&content));
    std::fs::write(&file2, bytes(&content)).unwrap();

    let info2 = PhpRef::new(Value::Null);
    let r = t
        .call(
            "getimagesize",
            vec![Value::from(file2.clone()), Value::Ref(info2.clone())],
        )
        .unwrap();
    let Value::Array(a) = &r else { panic!() };
    assert_eq!(a.get(&ArrayKey::Int(0)).unwrap().as_int(), Some(1));
    assert_eq!(a.get(&ArrayKey::Int(2)).unwrap().as_int(), Some(2));
    let Value::Array(i) = info2.get() else {
        panic!()
    };
    assert!(i.get_str(b"APP0").is_some());
    let app13 = i.get_str(b"APP13").expect("APP13").unref();

    let parsed = t.call("iptcparse", vec![app13]).unwrap();
    let parsed = dump_parse(&parsed);
    for (tag, string) in tags {
        let found = parsed
            .iter()
            .find(|(k, _)| k == tag)
            .unwrap_or_else(|| panic!("{tag}"));
        assert_eq!(found.1[0], string, "{tag}");
    }
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_file(&file2);
    assert_eq!(t.output(), "");
}

#[test]
fn iptcembed_bug70096_repeated_embedding_is_stable() {
    let mut t = TestVm::new();
    let file = scratch("bug70096.jpg");
    std::fs::write(&file, jpeg_1x1()).unwrap();
    let data =
        b"\x1C\x02x\x00\x0ATest image\x1C\x02t\x00\x22Copyright 2008-2009, The PHP Group".to_vec();
    let content1 = t
        .call(
            "iptcembed",
            vec![Value::from(data.clone()), Value::from(file.clone())],
        )
        .unwrap();
    std::fs::write(&file, bytes(&content1)).unwrap();
    let content2 = t
        .call(
            "iptcembed",
            vec![Value::from(data), Value::from(file.clone())],
        )
        .unwrap();
    let _ = std::fs::remove_file(&file);
    assert_eq!(bytes(&content1), bytes(&content2));
}

/// SOI, an empty APP0 and an (empty) SOS, as written by iptcembed_reentrant.
const TINY_JPEG: &[u8] = b"\xff\xd8\xff\xe0\x00\x02\xff\xda\x00\x02";

#[test]
fn iptcembed_spool_2_writes_to_the_output() {
    let file = scratch("reentrant.jpg");
    std::fs::write(&file, TINY_JPEG).unwrap();

    for buffered in [false, true] {
        let mut t = TestVm::new();
        if buffered {
            // Every byte is then a separate write to the output layer.
            t.vm.ob_start(None, 0, 0).unwrap();
        }
        let r = t
            .call(
                "iptcembed",
                vec![Value::str("AA"), Value::from(file.clone()), Value::Int(2)],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(true)));
        let out = if buffered {
            t.vm.ob_get_clean()
                .unwrap()
                .expect("buffer")
                .as_bytes()
                .to_vec()
        } else {
            t.out.take()
        };
        let start = out
            .windows(2)
            .position(|w| w == b"\xff\xed")
            .expect("APP13 marker");
        // APP13 length: header (28) + 2 data bytes ("AA" is even) + 2 length bytes.
        assert_eq!(u16::from_be_bytes([out[start + 2], out[start + 3]]), 30);
        assert_eq!(&out[start + 30..start + 32], b"AA");
        assert!(out.starts_with(b"\xff\xd8\xff\xe0\x00\x02"));
        assert!(out.ends_with(b"\xff\xda\x00\x02"));
    }
    let _ = std::fs::remove_file(&file);
}

#[test]
fn iptcembed_spool_1_returns_and_writes() {
    let file = scratch("spool1.jpg");
    std::fs::write(&file, TINY_JPEG).unwrap();
    let mut t = TestVm::new();
    let r = t
        .call(
            "iptcembed",
            vec![Value::str("abc"), Value::from(file.clone()), Value::Int(1)],
        )
        .unwrap();
    let _ = std::fs::remove_file(&file);
    let returned = bytes(&r);
    assert_eq!(t.out.take(), returned);
    // Odd data is padded with a NUL byte; the APP13 block carries the padded length.
    let start = returned.windows(2).position(|w| w == b"\xff\xed").unwrap();
    assert_eq!(
        u16::from_be_bytes([returned[start + 2], returned[start + 3]]),
        32
    );
    assert_eq!(&returned[start + 30..start + 34], b"abc\0");
}

#[test]
fn iptcembed_replaces_an_existing_app13() {
    // SOI, APP0, APP13 (old data), SOS.
    let mut jpeg = b"\xff\xd8\xff\xe0\x00\x02".to_vec();
    jpeg.extend_from_slice(b"\xff\xed\x00\x06old!");
    jpeg.extend_from_slice(b"\xff\xda\x00\x02");
    let file = scratch("app13.jpg");
    std::fs::write(&file, &jpeg).unwrap();
    let mut t = TestVm::new();
    let r = t
        .call(
            "iptcembed",
            vec![Value::str("NEW!"), Value::from(file.clone())],
        )
        .unwrap();
    let _ = std::fs::remove_file(&file);
    let out = bytes(&r);
    assert!(!out.windows(4).any(|w| w == b"old!"), "{out:?}");
    assert_eq!(out.windows(4).filter(|w| *w == b"NEW!").count(), 1);
    assert_eq!(out.windows(2).filter(|w| *w == b"\xff\xed").count(), 1);
}

#[test]
fn iptcembed_rejects_non_jpeg() {
    let file = scratch("notjpeg.bin");
    let mut t = TestVm::new();
    for content in [&b""[..], b"\xff", b"\xff\xd9", b"GIF89a"] {
        std::fs::write(&file, content).unwrap();
        let r = t
            .call(
                "iptcembed",
                vec![Value::str("A"), Value::from(file.clone())],
            )
            .unwrap();
        assert!(is_false(&r), "{content:?}");
    }
    let _ = std::fs::remove_file(&file);
}

#[test]
fn iptcembed_gh20582_spool_buffer_is_bounded() {
    // A FIFO reports st_size 0: the spool buffer is only
    // `len + sizeof(psheader) + 1024 + 1` bytes and must never be overrun.
    let mut stream = b"\xFF\xD8".to_vec();
    stream.extend_from_slice(b"\xFF\xE0\x00\x10JFIF");
    stream.extend_from_slice(&[0u8; 9]);
    stream.extend_from_slice(b"\xFF\xDA\x00\x08");
    stream.extend_from_slice(&[0u8; 6]);
    stream.extend_from_slice(&[b'A'; 5120]);

    let mut t = TestVm::new();
    let r = embed_stream(&mut t.vm, Cursor::new(stream.clone()), b"A", 0, 0).unwrap();
    let out = bytes(&r);
    assert_eq!(out.len(), 1 + 29 + 1024 + 1);
    assert!(out.starts_with(b"\xff\xd8\xff\xe0\x00\x10JFIF"));
    assert!(out.windows(14).any(|w| w == b"Photoshop 3.0\0"));
    assert!(out.ends_with(b"AAAA"));

    // With the real size the whole stream fits.
    let r = embed_stream(
        &mut t.vm,
        Cursor::new(stream.clone()),
        b"A",
        0,
        stream.len() as u64,
    )
    .unwrap();
    assert!(bytes(&r).len() > 5120);
}
