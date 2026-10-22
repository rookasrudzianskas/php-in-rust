//! mb_encode_numericentity / mb_decode_numericentity.

use super::*;

fn map(v: &[i64]) -> Value {
    Value::array(v.iter().map(|&i| Value::Int(i)).collect::<PhpArray>())
}

fn enc(t: &mut TestVm, st: &[u8], m: &[i64], e: &str, hexa: bool) -> String {
    text(&call(t, "mb_encode_numericentity", vec![b(st), map(m), s(e), Value::Bool(hexa)]))
}

fn dec(t: &mut TestVm, st: &[u8], m: &[i64], e: &str) -> Vec<u8> {
    bytes(&call(t, "mb_decode_numericentity", vec![b(st), map(m), s(e)]))
}

const FULL: [i64; 4] = [0, 0xFFFF, 0, 0xFFFF];

#[test]
fn encode() {
    let mut t = TestVm::new();
    // mb_encode_numericentity.phpt
    let conv = [0x0, 0x2FFFF, 0, 0xFFFF];
    let latin: String = (0xA1u32..=0xFF).filter_map(char::from_u32).collect();
    let dec_expected: String = (0xA1u32..=0xFF).map(|c| format!("&#{c};")).collect();
    let hex_expected: String = (0xA1u32..=0xFF).map(|c| format!("&#x{c:X};")).collect();
    assert_eq!(enc(&mut t, latin.as_bytes(), &conv, "UTF-8", false), dec_expected);
    assert_eq!(enc(&mut t, latin.as_bytes(), &conv, "UTF-8", true), hex_expected);
    assert_eq!(
        enc(&mut t, "aŒbœcŠdše€fg".as_bytes(), &[0xFF, 0x2FFFF, 0, 0xFFFF], "UTF-8", false),
        "a&#338;b&#339;c&#352;d&#353;e&#8364;fg"
    );
    assert_eq!(enc(&mut t, "föo".as_bytes(), &[], "UTF-8", false), "föo");
    assert_eq!(enc(&mut t, b"\x00", &[0, 1, 0, 0xFFFF], "UTF-8", false), "&#0;");
    assert_eq!(enc(&mut t, b"\x00", &[0, 1, 0, 0xFFFF], "UTF-8", true), "&#x0;");
    let odd = [0, 0, 0, 0, 100, 50, 0, 0xFFFF];
    assert_eq!(enc(&mut t, "föo".as_bytes(), &odd, "UTF-8", false), "föo");
    assert_eq!(enc(&mut t, b"\x01\x02\x03", &[0, 10, 1000, 0xFFFF], "UTF-8", false), "&#1001;&#1002;&#1003;");
    assert_eq!(enc(&mut t, b"\x01\x02\x03", &[0, 10, 1000, 0xFFFF], "UTF-8", true), "&#x3E9;&#x3EA;&#x3EB;");
    assert_eq!(
        enc(&mut t, b"\x01\x02\x03", &[0, 10, -100000, 0xFFFFFF], "UTF-8", false),
        "&#16677217;&#16677218;&#16677219;"
    );
    assert_eq!(
        enc(&mut t, b"\x01\x02\x03", &[0, 10, -100000, 0xFFFFFF], "UTF-8", true),
        "&#xFE7961;&#xFE7962;&#xFE7963;"
    );
    assert_eq!(enc(&mut t, "föo".as_bytes(), &[0, 1000, 0, 0x123], "UTF-8", false), "&#34;&#34;&#35;");
    assert_eq!(enc(&mut t, "föo".as_bytes(), &[0, 1000, 0, 0x123], "UTF-8", true), "&#x22;&#x22;&#x23;");
    let iso2022jp = b"\x1B$B\x21\x21!r\x1B(BABC";
    assert_eq!(enc(&mut t, iso2022jp, &FULL, "ISO-2022-JP", false), "&#12288;&#163;&#65;&#66;&#67;");
    assert_eq!(enc(&mut t, iso2022jp, &FULL, "ISO-2022-JP", true), "&#x3000;&#xA3;&#x41;&#x42;&#x43;");
    assert_eq!(enc(&mut t, b"m", &[0x2b, 0x2d4, 0x75656500, 0x656d2c53], "ASCII", false), "&#1701127233;");
    // mb_encode_numericentity_large_ints.phpt
    assert_eq!(
        enc(&mut t, b"\xFF", &[0xffffffff, 0xffffffff, 0x540a0af7, 0x5a485054], "ASCII", true),
        "&#x50080054;"
    );
    // mb_encode_numericentity_errors.phpt
    for f in ["mb_encode_numericentity", "mb_decode_numericentity"] {
        assert_eq!(
            err(&mut t, f, vec![s("str"), map(&[0xFF, 0x2FFFF, 0]), s("UTF-8")]),
            format!("ValueError: {f}(): Argument #2 ($map) must have a multiple of 4 elements")
        );
        let bad = Value::array(PhpArray::from_values(vec![
            Value::Int(0xFF),
            s("not an int"),
            Value::Int(0),
            Value::Int(0),
        ]));
        assert_eq!(
            err(&mut t, f, vec![s("str"), bad, s("UTF-8")]),
            format!("ValueError: {f}(): Argument #2 ($map) must only be composed of values of type int")
        );
    }
    // mb_encode_numericentity_references.phpt
    let r = Value::Ref(php_runtime::PhpRef::new(Value::Int(0)));
    let m = Value::array(PhpArray::from_values(vec![r, Value::Int(0x1FFFFF), Value::Int(0), Value::Int(0x10FFFF)]));
    assert_eq!(text(&call(&mut t, "mb_encode_numericentity", vec![s(""), m, s("utf8")])), "");
}

#[test]
fn decode() {
    let mut t = TestVm::new();
    // mb_decode_numericentity.phpt
    let conv = [0x0, 0x2FFFF, 0, 0xFFFF];
    let latin: String = (0xA1u32..=0xFF).filter_map(char::from_u32).collect();
    let entities: String = (0xA1u32..=0xFF).map(|c| format!("&#{c};")).collect();
    assert_eq!(dec(&mut t, entities.as_bytes(), &conv, "UTF-8"), latin.as_bytes());
    assert_eq!(
        dec(&mut t, b"a&#338;b&#339;c&#352;d&#353;e&#8364;fg", &conv, "UTF-8"),
        "aŒbœcŠdše€fg".as_bytes()
    );
    for (input, out) in [
        (&b"&#1000000000"[..], &b"&#1000000000"[..]),
        (b"&#9000000000", b"&#9000000000"),
        (b"&#10000000000", b"&#10000000000"),
        (b"&#100000000000", b"&#100000000000"),
        (b"&#000000000000", b"&#000000000000"),
        (b"&#00000000000", b"&#00000000000"),
        (b"&#0000000000", b"\x00"),
        (b"&#000000000", b"\x00"),
        (b"&#x0000000", b"\x00"),
        (b"&#x00000000", b"\x00"),
        (b"&#x10000", b"\xf0\x90\x80\x80"),
        (b"&#x000000000", b"&#x000000000"),
    ] {
        assert_eq!(dec(&mut t, input, &conv, "UTF-8"), out, "{}", String::from_utf8_lossy(input));
    }
    let ucs4 = call(&mut t, "mb_convert_encoding", vec![s("&#12345678;"), s("UCS-4"), s("ASCII")]);
    let r = call(&mut t, "mb_decode_numericentity", vec![ucs4, map(&[0, 0x7FFFFFFF, 0, 0x7FFFFFFF]), s("UCS-4")]);
    assert_eq!(hex(&r), "00bc614e");
    assert_eq!(dec(&mut t, b"f&ouml;o", &[], "UTF-8"), b"f&ouml;o");
    assert_eq!(dec(&mut t, b"&#0;", &[0, 1, 0, 0xFFFF], "UTF-8"), b"\x00");
    assert_eq!(dec(&mut t, b"&#x0;", &[0, 1, 0, 0xFFFF], "UTF-8"), b"\x00");
    assert_eq!(dec(&mut t, "föo".as_bytes(), &[0, 0, 0, 0, 100, 50, 0, 0xFFFF], "UTF-8"), "föo".as_bytes());
    assert_eq!(dec(&mut t, b"&#1001;&#1002;&#1003;", &[0, 10, 1000, 0xFFFF], "UTF-8"), b"\x01\x02\x03");
    assert_eq!(dec(&mut t, b"&#x3E9;&#x3EA;&#x3EB;", &[0, 10, 1000, 0xFFFF], "UTF-8"), b"\x01\x02\x03");
    assert_eq!(dec(&mut t, b"&#123a;", &FULL, "UTF-8"), b"{a;");
    for (input, out, m) in [
        (&b"&#0000000065;"[..], &b"A"[..], FULL),
        (b"&#00000000165;", b"&#00000000165;", FULL),
        (b"&#x00000041;", b"A", FULL),
        (b"&#x000000141;", b"&#x000000141;", FULL),
        (b"&", b"&", FULL),
        (b"&&#65,", b"&A,", FULL),
        (b"&#&#x32;", b"&#2", FULL),
        (b"&#x&#x32;", b"&#x2", FULL),
        (b"&#x;", b"&#x;", FULL),
        (b"&#65&#65;", b"AA", FULL),
        (b"&#x32&#x32;", b"22", FULL),
        (b"&#10000000000&#65;", b"&#10000000000A", FULL),
        (b"&#x111111111&#65;", b"&#x111111111A", FULL),
        (b"&#0&#65;", b"&#0A", [0x1, 0xFFFF, 0, 0xFFFF]),
        (b"&#x0&#65;", b"&#x0A", [0x1, 0xFFFF, 0, 0xFFFF]),
    ] {
        assert_eq!(dec(&mut t, input, &m, "ASCII"), out, "{}", String::from_utf8_lossy(input));
    }
    // Entities split across buffers.
    for i in 0..256 {
        let pad = "a".repeat(i);
        for (tail, out) in [("&#ZZZ", "&#ZZZ"), ("&#xZZZ", "&#xZZZ"), ("&#65", "A"), ("&#x41", "A")] {
            let input = format!("{pad}{tail}");
            assert_eq!(dec(&mut t, input.as_bytes(), &FULL, "UTF-8"), format!("{pad}{out}").as_bytes());
        }
    }
    for i in 12..256 {
        let st = format!("&#{}65", "0".repeat(i));
        assert_eq!(dec(&mut t, st.as_bytes(), &FULL, "UTF-8"), st.as_bytes());
        let st = format!("&#x{}41", "0".repeat(i));
        assert_eq!(dec(&mut t, st.as_bytes(), &FULL, "UTF-8"), st.as_bytes());
    }
    // mb_decode_numericentity_large_ints.phpt
    let all = [0, 0xFFFFFFFF, 0, 0xFFFFFFFF];
    let u = call(&mut t, "mb_convert_encoding", vec![s("&#1000000000&#65;"), s("UCS-4BE"), s("ASCII")]);
    let r = call(&mut t, "mb_decode_numericentity", vec![u, map(&all), s("UCS-4BE")]);
    assert_eq!(hex(&r), "3b9aca0000000041");
    let u = call(&mut t, "mb_convert_encoding", vec![s("&#x11111111&#65;"), s("UCS-4BE"), s("ASCII")]);
    let r = call(&mut t, "mb_decode_numericentity", vec![u, map(&all), s("UCS-4BE")]);
    assert_eq!(hex(&r), "1111111100000041");
    assert_eq!(dec(&mut t, b"&#7001492542&#65;", &all, "ASCII"), b"&#7001492542A");
    assert_eq!(dec(&mut t, b"&#xe;", &[0xFFFFFF86, 0xFFFFFFFF, 0xF, 0xFC015448], "HZ"), b"?");
    assert_eq!(dec(&mut t, b"&&&", &[0x20FF37FF, 0x7202F569, 0xC4090023, 0xF160], "JIS"), b"&&&");
    assert_eq!(dec(&mut t, b"&#7,", &[0x22FFFF11, 0xBF111189, 0x67726511, 0x1161E719], "ASCII"), b"?,");
}
