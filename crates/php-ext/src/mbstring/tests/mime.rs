//! mb_encode_mimeheader / mb_decode_mimeheader.

use super::*;

fn encode(t: &mut TestVm, st: &[u8], charset: &str, te: &str, lf: Option<&[u8]>, indent: Option<i64>) -> Value {
    let mut args = vec![b(st), s(charset), s(te)];
    if let Some(lf) = lf {
        args.push(b(lf));
    }
    if let Some(i) = indent {
        if args.len() == 3 {
            args.push(s("\r\n"));
        }
        args.push(Value::Int(i));
    }
    call(t, "mb_encode_mimeheader", args)
}

fn base64(s: &str) -> Vec<u8> {
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut bits, mut acc) = (0u32, 0u32);
    for c in s.bytes().filter(|&c| c != b'=') {
        let v = table.iter().position(|&t| t == c).unwrap() as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

const GREEK: &str = "zpHPhc+Ez4wgzrXOr869zrHOuSDOtc67zrvOt869zrnOus+MIM66zrXOr868zrXOvc6/LiAwMTIzNDU2Nzg5Lg==";

#[test]
fn encode_basic() {
    // mb_encode_mimeheader_basic.phpt
    let mut t = TestVm::new();
    let english = b"This is an English string. 0123456789";
    for te in ["B", "Q"] {
        assert_eq!(bytes(&encode(&mut t, english, "UTF-8", te, None, None)), english);
    }
    call(&mut t, "mb_internal_encoding", vec![s("utf-8")]);
    let jp = base64("5pel5pys6Kqe44OG44Kt44K544OI44Gn44GZ44CC");
    assert_eq!(text(&encode(&mut t, &jp, "UTF-8", "B", None, None)), "=?UTF-8?B?5pel5pys6Kqe44OG44Kt44K544OI44Gn44GZ44CC?=");
    assert_eq!(
        text(&encode(&mut t, &jp, "UTF-8", "Q", None, None)),
        "=?UTF-8?Q?=E6=97=A5=E6=9C=AC=E8=AA=9E=E3=83=86=E3=82=AD=E3=82=B9=E3=83=88?=\r\n =?UTF-8?Q?=E3=81=A7=E3=81=99=E3=80=82?="
    );
    let greek = base64(GREEK);
    assert_eq!(
        text(&encode(&mut t, &greek, "UTF-8", "B", None, None)),
        "=?UTF-8?B?zpHPhc+Ez4wgzrXOr869zrHOuSDOtc67zrvOt869zrnOus+MIM66zrXOr868?=\r\n =?UTF-8?B?zrXOvc6/LiAwMTIzNDU2Nzg5Lg==?="
    );
    assert_eq!(
        text(&encode(&mut t, &greek, "UTF-8", "Q", None, None)),
        "=?UTF-8?Q?=CE=91=CF=85=CF=84=CF=8C=20=CE=B5=CE=AF=CE=BD=CE=B1=CE=B9=20?=\r\n =?UTF-8?Q?=CE=B5=CE=BB=CE=BB=CE=B7=CE=BD=CE=B9=CE=BA=CF=8C=20=CE=BA=CE=B5?=\r\n =?UTF-8?Q?=CE=AF=CE=BC=CE=B5=CE=BD=CE=BF=2E=20=30=31=32=33=34=35=36=37=38?=\r\n =?UTF-8?Q?=39=2E?="
    );
}

#[test]
fn encode_japanese() {
    // mb_encode_mimeheader_basic2.phpt / basic3.phpt
    let mut t = TestVm::new();
    let inputs = [
        ("SJIS", base64("k/qWe4zqg2WDTINYg2eCxYK3gUIwMTIzNIJUglWCVoJXgliBQg==")),
        ("JIS", base64("GyRCRnxLXDhsJUYlLSU5JUgkRyQ5ISMbKEIwMTIzNBskQiM1IzYjNyM4IzkhIxsoQg==")),
        ("EUC_JP", base64("xvzL3LjspcalraW5pcikx6S5oaMwMTIzNKO1o7ajt6O4o7mhow==")),
    ];
    for (lang, input) in &inputs {
        call(&mut t, "mb_internal_encoding", vec![s(lang)]);
        assert_eq!(
            text(&encode(&mut t, input, "UTF-8", "B", None, None)),
            "=?UTF-8?B?5pel5pys6Kqe44OG44Kt44K544OI44Gn44GZ44CCMDEyMzTvvJXvvJbvvJc=?=\r\n =?UTF-8?B?77yY77yZ44CC?=",
            "{lang}"
        );
        assert_eq!(
            text(&encode(&mut t, input, "UTF-8", "Q", None, None)),
            "=?UTF-8?Q?=E6=97=A5=E6=9C=AC=E8=AA=9E=E3=83=86=E3=82=AD=E3=82=B9=E3=83=88?=\r\n =?UTF-8?Q?=E3=81=A7=E3=81=99=E3=80=82=30=31=32=33=34=EF=BC=95=EF=BC=96?=\r\n =?UTF-8?Q?=EF=BC=97=EF=BC=98=EF=BC=99=E3=80=82?=",
            "{lang}"
        );
    }
    let expected = [
        (
            "=?Shift_JIS?B?k/qWe4zqg2WDTINYg2eCxYK3gUIwMTIzNIJUglWCVoJXgliBQg==?=",
            "=?Shift_JIS?Q?=93=FA=96=7B=8C=EA=83e=83L=83X=83g=82=C5=82=B7=81B=30=31=32?=\r\n =?Shift_JIS?Q?=33=34=82T=82U=82V=82W=82X=81B?=",
        ),
        (
            "=?ISO-2022-JP?B?GyRCRnxLXDhsJUYlLSU5JUgkRyQ5ISMbKEIwMTIzNBskQiM1IzYbKEI=?=\r\n =?ISO-2022-JP?B?GyRCIzcjOCM5ISMbKEI=?=",
            "=?ISO-2022-JP?Q?=1B=24BF=7CK=5C=38l=25F=25-=25=39=25H=24G=24=39=1B=28B?=\r\n =?ISO-2022-JP?Q?=1B=24B!=23=1B=28B=30=31=32=33=34=1B=24B=23=35=1B=28B?=\r\n =?ISO-2022-JP?Q?=1B=24B=23=36=23=37=23=38=23=39!=23=1B=28B?=",
        ),
        (
            "=?EUC-JP?B?xvzL3LjspcalraW5pcikx6S5oaMwMTIzNKO1o7ajt6O4o7mhow==?=",
            "=?EUC-JP?Q?=C6=FC=CB=DC=B8=EC=A5=C6=A5=AD=A5=B9=A5=C8=A4=C7=A4=B9=A1=A3?=\r\n =?EUC-JP?Q?=30=31=32=33=34=A3=B5=A3=B6=A3=B7=A3=B8=A3=B9=A1=A3?=",
        ),
    ];
    for ((lang, input), (eb, eq)) in inputs.iter().zip(expected) {
        call(&mut t, "mb_internal_encoding", vec![s(lang)]);
        assert_eq!(text(&encode(&mut t, input, lang, "B", None, None)), eb, "{lang}");
        assert_eq!(text(&encode(&mut t, input, lang, "Q", None, None)), eq, "{lang}");
    }
}

#[test]
fn encode_edge_cases() {
    // mb_encode_mimeheader_basic4.phpt, compared with the whole expected output
    let mut t = TestVm::new();
    t.vm.ini_set_system("error_reporting", "24575");
    let mut out = String::new();
    fn d(t: &mut TestVm, out: &mut String, st: &[u8], cs: &str, te: &str, lf: Option<&[u8]>, indent: Option<i64>) {
        let v = encode(t, st, cs, te, lf, indent);
        out.push_str(&dump(t, v));
    }
    call(&mut t, "mb_internal_encoding", vec![s("UTF-8")]);
    d(&mut t, &mut out, b"", "UTF-8", "Q", None, None);
    d(&mut t, &mut out, b"abc\x00abc", "UTF-8", "Q", None, None);
    d(&mut t, &mut out, b"\xE2", "UTF-8", "B", None, None);
    for c in [b"?", b"=", b"_"] {
        d(&mut t, &mut out, c, "ASCII", "B", None, None);
        d(&mut t, &mut out, c, "ASCII", "Q", None, None);
    }
    d(&mut t, &mut out, b"\x7f", "ASCII", "B", Some(b""), None);
    for st in [&b" "[..], b"   ", b"ab  ab  "] {
        d(&mut t, &mut out, st, "ASCII", "B", None, None);
        d(&mut t, &mut out, st, "ASCII", "Q", None, None);
    }
    d(&mut t, &mut out, b"` ", "HZ", "B", Some(b""), None);
    d(&mut t, &mut out, b"S ", "ASCII", "Q", Some(b""), Some(73));
    d(&mut t, &mut out, b"S4", "ASCII", "B", Some(b"\n"), Some(73));
    d(&mut t, &mut out, b"S4", "ASCII", "Q", Some(b"\n"), Some(73));
    d(&mut t, &mut out, b"24\x0a", "UCS-4", "Q", Some(b""), Some(29));
    d(&mut t, &mut out, b"o\x20\x00", "ASCII", "B", None, None);
    d(&mut t, &mut out, b"\x00\x11\x01\x00\x00\x00\x00\x00\x00\x00", "UCS-4", "B", None, None);
    d(&mut t, &mut out, b"\x01\x00\xcb\xcb\xcb\xcb\xcb\xcb=\xcb\xcb\xcb=?\x01\x00a\x00\x00\xcb\xcb\xcb=?\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb?4?4\xcb\xcb\xcb\xcb\xcb=?\x01\x00\x00\x00\x01\x00\x00\x06\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb=?\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb?4\xcb\xcb\xcb\xcb\xcb?4", "UCS-2", "B", Some(b""), None);
    d(&mut t, &mut out, b"?", "ASCII", "Q", Some(b"=aaaaaa=?"), Some(52));
    out.push_str("string(9) \"=aaaaaa=?\"\n");
    d(&mut t, &mut out, b",\x20o\x00\x01\x00\x00(", "JIS", "Q", Some(b""), Some(40));
    d(&mut t, &mut out, b"\xff", "ASCII", "Q", Some(b"\x00"), Some(54));
    d(&mut t, &mut out, b"\xff~H~\xe0\xea\x00\x00\xff\xff\xff\xff\xff>\x00\x00\x00\x00", "HZ", "Q", Some(b""), Some(71));
    d(&mut t, &mut out, b"yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyBIG5", "BIG-5", "B", None, None);
    d(&mut t, &mut out, b"\x20\x20\x20\x202\x20\x20\x20sssssssssssssssssssssssssss\x20\x20\x20\x20W\x20\x20\x20\x20\x20\x20W\x20\x20\x20\x20\xb9S\x01\x00\xf0`\x00\x00\x20\x20\x20\x20mSCII\xee\x20\x20\x20\x20mSCII\xeeI\xee", "ArmSCII-8", "B", Some(b""), None);
    let spaces80 = format!("{}\x00", " ".repeat(80));
    d(&mut t, &mut out, spaces80.as_bytes(), "CP936", "Q", Some(b""), None);
    let ones = format!("  {}", "1".repeat(73));
    d(&mut t, &mut out, ones.as_bytes(), "ASCII", "Q", Some(b""), None);
    d(&mut t, &mut out, b"\xe7\xad\xb5", "HZ", "Q", Some(b""), Some(44));
    let gb1 = format!("2{}!3", " ".repeat(74));
    d(&mut t, &mut out, gb1.as_bytes(), "GB18030", "Q", Some(b""), None);
    let gb2 = format!("{}!3 ", " ".repeat(72));
    d(&mut t, &mut out, gb2.as_bytes(), "GB18030", "Q", Some(b""), None);
    let gb3 = format!("2{}!3", " ".repeat(89));
    d(&mut t, &mut out, gb3.as_bytes(), "GB18030", "Q", Some(b""), None);
    d(&mut t, &mut out, b",9868949,9868978,9869015,9689100,9869121,9869615,9870690,9867116,98558119861183. ", "utf-8", "B", None, None);
    let xx = format!("xx {} ", "A".repeat(81));
    d(&mut t, &mut out, xx.as_bytes(), "utf-8", "B", None, None);
    call(&mut t, "mb_internal_encoding", vec![s("MacJapanese")]);
    d(&mut t, &mut out, b"ne\xf6\xff\xff\xffs\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff1\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff1", "CP50220", "B", Some(b"A"), Some(44));
    call(&mut t, "mb_internal_encoding", vec![s("Quoted-Printable")]);
    let e = err(&mut t, "mb_encode_mimeheader", vec![s("x"), s("Quoted-Printable"), s("B"), s(""), Value::Int(184)]);
    out.push_str(&e);
    out.push_str("\nDone");
    assert_eq!(normalize(&out), normalize(&phpt_section("mb_encode_mimeheader_basic4", "EXPECT")));
}

#[test]
fn encode_indent_and_linefeeds() {
    // mb_encode_mimeheader_indent.phpt
    let mut t = TestVm::new();
    t.vm.ini_set_system("internal_encoding", "utf-8");
    let greek = base64(GREEK);
    let mut out = String::from("*** Testing mb_encode_mimeheader() : indent ***\n");
    for i in 0..100 {
        out.push_str(&format!("\n-- Iteration {i} --\n"));
        for te in ["B", "Q"] {
            let v = encode(&mut t, &greek, "utf-8", te, Some(b"\r\n"), Some(i));
            out.push_str(&dump(&mut t, v));
        }
    }
    out.push_str("Done");
    assert_eq!(normalize(&out), normalize(&phpt_section("mb_encode_mimeheader_indent", "EXPECT")));
    // mb_encode_mimeheader_variation6.phpt
    let r = encode(&mut t, &greek, "utf-8", "B", Some(b"---"), None);
    assert_eq!(
        text(&r),
        "=?UTF-8?B?zpHPhc+Ez4wgzrXOr869zrHOuSDOtc67zrvOt869zrnOus+MIM66zrXOr868?=--- =?UTF-8?B?zrXOvc6/LiAwMTIzNDU2Nzg5Lg==?="
    );
    assert_eq!(bytes(&encode(&mut t, &greek, "utf-8", "B", Some(b"\n"), None)).len(), 114);
    // mb_encode_mimeheader_crash.phpt
    assert_eq!(
        err(&mut t, "mb_encode_mimeheader", vec![s("abc"), s("UTF7-IMAP"), s("Q")]),
        "ValueError: mb_encode_mimeheader(): Argument #2 ($charset) \"UTF7-IMAP\" cannot be used for MIME header encoding"
    );
}

#[test]
fn decode() {
    let mut t = TestVm::new();
    t.vm.ini_set_system("internal_encoding", "utf-8");
    // mb_decode_mimeheader_basic.phpt
    let expect = "e697a5e69cace8aa9ee38386e382ade382b9e38388e381a7e38199e38082";
    let dec = |t: &mut TestVm, st: &[u8]| hex(&call(t, "mb_decode_mimeheader", vec![b(st)]));
    assert_eq!(dec(&mut t, b"=?UTF-8?b?5pel5pys6Kqe44OG44Kt44K544OI44Gn44GZ44CC?="), expect);
    assert_eq!(
        dec(&mut t, b"=?UTF-8?Q?=E6=97=A5=E6=9C=AC=E8=AA=9E=E3=83=86=E3=82=AD=E3=82=B9=E3=83=88?=\n=?UTF-8?Q?=E3=81=A7=E3=81=99=E3=80=82?="),
        expect
    );
    // mb_decode_mimeheader_variation2.phpt
    let long = "e697a5e69cace8aa9ee38386e382ade382b9e38388e381a7e38199e380823031323334efbc95efbc96efbc97efbc98efbc99e38082";
    for w in [
        &b"=?Shift_JIS?B?k/qWe4zqg2WDTINYg2eCxYK3gUIwMTIzNIJUglWCVoJXgliBQg==?="[..],
        b"=?Shift_JIS?Q?=93=FA=96=7B=8C=EA=83e=83L=83X=83g=82=C5=82=B7=81B=30=31=32?=\n=?Shift_JIS?Q?=33=34=82T=82U=82V=82W=82X=81B?=",
        b"=?ISO-2022-JP?B?GyRCRnxLXDhsJUYlLSU5JUgkRyQ5ISMbKEIwMTIzNBskQiM1IzYbKEI=?=\n=?ISO-2022-JP?B?GyRCIzcjOCM5ISMbKEI=?=",
        b"=?ISO-2022-JP?Q?=1B=24BF=7CK=5C=38l=25F=25-=25=39=25H=24G=24=39=1B=28B?=\n=?ISO-2022-JP?Q?=1B=24B!=23=1B=28B=30=31=32=33=34=1B=24B=23=35=1B=28B?=\n=?ISO-2022-JP?Q?=1B=24B=23=36=23=37=23=38=23=39!=23=1B=28B?=",
        b"=?EUC-JP?B?xvzL3LjspcalraW5pcikx6S5oaMwMTIzNKO1o7ajt6O4o7mhow==?=",
        b"=?EUC-JP?Q?=C6=FC=CB=DC=B8=EC=A5=C6=A5=AD=A5=B9=A5=C8=A4=C7=A4=B9=A1=A3?=\n=?EUC-JP?Q?=30=31=32=33=34=A3=B5=A3=B6=A3=B7=A3=B8=A3=B9=A1=A3?=",
    ] {
        assert_eq!(dec(&mut t, w), long, "{}", String::from_utf8_lossy(w));
    }
    // mb_decode_mimeheader_variation5.phpt
    for (input, out) in [
        ("=?UTF-8?Q?abc?=", "abc"),
        ("=?UTF-8?Q?abc_def?=", "abc def"),
        ("_=?UTF-8?Q?abc_def?=_", "_abc def_"),
        ("=?UTF-8?Q?__=E6=B1=89=E5=AD=97__?=", "  汉字  "),
        ("=?UTF-8?Q?=5F?=", "_"),
    ] {
        assert_eq!(text(&call(&mut t, "mb_decode_mimeheader", vec![s(input)])), out);
    }
    // mb_decode_mimeheader_variation3.phpt
    call(&mut t, "mb_internal_encoding", vec![s("iso-8859-7")]);
    assert_eq!(
        dec(&mut t, b"=?UTF-8?B?zrHOss6zzrTOtc62zrfOuM65zrrOu868zr3Ovs6/z4DPgc+Dz4TPhc+Gz4fPiM+J?="),
        "e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f3f4f5f6f7f8f9"
    );
}

#[test]
fn decode_edge_cases() {
    // mb_decode_mimeheader_variation4.phpt
    let mut t = TestVm::new();
    let cases: Vec<(&str, &[u8], &str)> = vec![
        ("UCS-2", b"2,\rGCG\xb3GS", "0032002c0020004700430047003f00470053"),
        ("ASCII", b"\n8i", "203869"),
        ("ASCII", b"\r", ""),
        ("ASCII", b"\n ", ""),
        ("ASCII", b",\x13@=,", "2c13403d2c"),
        ("ASCII", b"=?I?B??=", "3d3f493f423f3f3d"),
        ("ISO-8859-7", b"=?=\x20?=?R\xb7=?=\x20?\x8b\x00====?===??=\xc5UC-R\xb7=?=\x20?=?=====?=\x20?======?======?=\x20?======?===??=\xc5UC-KR\xb7=?=\x20?=?===?==?\x0a",
            "3d3f3d203f3d3f523f3d3f3d203f3f003d3d3d3d3f3d3d3d3f3f3d3f55432d523f3d3f3d203f3d3f3d3d3d3d3d3f3d203f3d3d3d3d3d3d3f3d3d3d3d3d3d3f3d203f3d3d3d3d3d3d3f3d3d3d3f3f3d3f55432d4b523f3d3f3d203f3d3f3d3d3d3f3d3d3f"),
        ("UCS-2", b"=?\xc2\x86tf7,U\x01\x00`@\x00\x04|\xf1D\x18\x00\x00\x00v\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xff\x13f7,U&\x00S\x01\x00\x17,D\xcb\xcb\xcb\xcb\xcb\xcb\xcb\x01\x00\x00\x14\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\xcb\x00\x11\x00\x00\x00\x00\x00\x00\x0a",
            "003d003f003f003f007400660037002c0055000100000060004000000004007c003f004400180000000000000076003f003f003f003f003f003f003f003f003f003f003f003f001300660037002c0055002600000053000100000017002c0044003f003f003f003f003f003f003f0001000000000014003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f003f00000011000000000000000000000000"),
        ("ASCII", b"=?us?B?", ""),
        ("ASCII", b"=?us?B?-", "3f"),
        ("ASCII", b"=?us?B?-s", "3f"),
        ("ASCII", b"=?us?B??", ""),
        ("ASCII", b"=?us?B?\x00?=", "3f"),
        ("ASCII", b"=?=?hz?b?", "3d3f"),
        ("ASCII", b"=?==?hz?b?", "3d3f3d"),
        ("ASCII", b"=?,=?hz?b?", "3d3f2c"),
        ("UUENCODE", b"", "626567696e20303634342066696c656e616d650a20"),
        ("SJIS-Mobile#SOFTBANK", b"6", "36"),
        ("SJIS-Mobile#KDDI", b"6", "36"),
        ("SJIS-Mobile#DOCOMO", b"6", "36"),
    ];
    for (enc, input, expected) in cases {
        call(&mut t, "mb_internal_encoding", vec![s(enc)]);
        let r = call(&mut t, "mb_decode_mimeheader", vec![b(input)]);
        assert_eq!(hex(&r), expected, "{enc} {}", String::from_utf8_lossy(input));
    }
}
