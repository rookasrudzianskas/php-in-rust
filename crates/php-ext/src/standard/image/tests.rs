//! Unit tests for image.c. Most of them replay a `.phpt` script from
//! `tests/phpt/ext/standard/tests/image` against the natives and compare a
//! `var_dump` rendering of the result with the file's `--EXPECT--` /
//! `--EXPECTF--` section, so the expectations come straight from php-src.

use std::collections::HashSet;

use php_runtime::{ArrayKey, PhpRef, PhpStr, Value};

use crate::testing::{TestVm, thrown};

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/phpt/ext/standard/tests/image"
);

fn path(name: &str) -> String {
    format!("{DIR}/{name}")
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(path(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

// ---------------------------------------------------------------------------
// .phpt expectations
// ---------------------------------------------------------------------------

/// The `--EXPECT--` / `--EXPECTF--` body of `<name>.phpt` and whether it is a pattern.
fn phpt_expect(name: &str) -> (String, bool) {
    let text = std::fs::read_to_string(format!("{DIR}/{name}.phpt")).unwrap();
    let mut body = String::new();
    let mut in_section = None;
    for line in text.split_inclusive('\n') {
        let l = line.trim_end_matches(['\r', '\n']);
        let is_header = l.len() > 4
            && l.starts_with("--")
            && l.ends_with("--")
            && l[2..l.len() - 2]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_');
        if is_header {
            if in_section.is_some() {
                break;
            }
            match l {
                "--EXPECT--" => in_section = Some(false),
                "--EXPECTF--" => in_section = Some(true),
                _ => {}
            }
            continue;
        }
        if in_section.is_some() {
            body.push_str(line);
        }
    }
    (
        body.trim().to_string(),
        in_section.unwrap_or_else(|| panic!("{name}: no expectation")),
    )
}

#[derive(Debug)]
enum Tok {
    Lit(char),
    Digits,
    Str,
    OptStr,
    Any,
    OptAny,
    Space,
    DirSep,
    Float,
    Hex,
    Char,
}

fn tokenize(pat: &str) -> Vec<Tok> {
    let chars: Vec<char> = pat.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            let t = match chars[i + 1] {
                'd' | 'i' => Some(Tok::Digits),
                's' => Some(Tok::Str),
                'S' => Some(Tok::OptStr),
                'a' => Some(Tok::Any),
                'A' => Some(Tok::OptAny),
                'w' => Some(Tok::Space),
                'e' => Some(Tok::DirSep),
                'f' => Some(Tok::Float),
                'x' => Some(Tok::Hex),
                'c' => Some(Tok::Char),
                '%' => Some(Tok::Lit('%')),
                _ => None,
            };
            if let Some(t) = t {
                toks.push(t);
                i += 2;
                continue;
            }
        }
        toks.push(Tok::Lit(chars[i]));
        i += 1;
    }
    toks
}

fn matches_at(
    toks: &[Tok],
    ti: usize,
    s: &[char],
    si: usize,
    memo: &mut HashSet<(usize, usize)>,
) -> bool {
    if ti == toks.len() {
        return si == s.len();
    }
    if memo.contains(&(ti, si)) {
        return false;
    }
    let ok = match &toks[ti] {
        Tok::Lit(c) => si < s.len() && s[si] == *c && matches_at(toks, ti + 1, s, si + 1, memo),
        Tok::Char => si < s.len() && matches_at(toks, ti + 1, s, si + 1, memo),
        Tok::DirSep => {
            si < s.len()
                && (s[si] == '/' || s[si] == '\\')
                && matches_at(toks, ti + 1, s, si + 1, memo)
        }
        tok => {
            let (min, pred): (usize, fn(char) -> bool) = match tok {
                Tok::Digits => (1, |c| c.is_ascii_digit() || c == '-' || c == '+'),
                Tok::Str => (1, |c| c != '\n' && c != '\r'),
                Tok::OptStr => (0, |c| c != '\n' && c != '\r'),
                Tok::Any => (1, |_| true),
                Tok::OptAny => (0, |_| true),
                Tok::Space => (0, |c| c.is_whitespace()),
                Tok::Float => (1, |c| {
                    c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')
                }),
                Tok::Hex => (1, |c| c.is_ascii_hexdigit()),
                _ => unreachable!(),
            };
            let mut max = 0;
            while si + max < s.len() && pred(s[si + max]) {
                max += 1;
            }
            (min..=max)
                .rev()
                .any(|n| matches_at(toks, ti + 1, s, si + n, memo))
        }
    };
    if !ok {
        memo.insert((ti, si));
    }
    ok
}

fn expectf_matches(pattern: &str, output: &str) -> bool {
    let toks = tokenize(pattern);
    let s: Vec<char> = output.chars().collect();
    matches_at(&toks, 0, &s, 0, &mut HashSet::new())
}

// ---------------------------------------------------------------------------
// var_dump
// ---------------------------------------------------------------------------

fn var_dump(v: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match &*v.deref() {
        Value::Null | Value::Undef => out.push_str(&format!("{pad}NULL\n")),
        Value::Bool(b) => out.push_str(&format!("{pad}bool({b})\n")),
        Value::Int(i) => out.push_str(&format!("{pad}int({i})\n")),
        Value::String(s) => out.push_str(&format!(
            "{pad}string({}) \"{}\"\n",
            s.len(),
            String::from_utf8_lossy(s.as_bytes())
        )),
        Value::Array(a) => {
            out.push_str(&format!("{pad}array({}) {{\n", a.len()));
            for (k, v) in a.iter() {
                match k {
                    ArrayKey::Int(i) => out.push_str(&format!("{pad}  [{i}]=>\n")),
                    ArrayKey::Str(s) => out.push_str(&format!(
                        "{pad}  [\"{}\"]=>\n",
                        String::from_utf8_lossy(s.as_bytes())
                    )),
                }
                var_dump(v, indent + 2, out);
            }
            out.push_str(&format!("{pad}}}\n"));
        }
        other => panic!("var_dump: unsupported {other:?}"),
    }
}

/// A script run: the text a PHP script would have printed (echo, var_dump and
/// displayed warnings, in order).
struct Run {
    vm: TestVm,
    text: String,
}

impl Run {
    fn new() -> Run {
        Run {
            vm: TestVm::new(),
            text: String::new(),
        }
    }

    /// Pretends the script runs in `file` (for `%s%ebug13213.php` patterns).
    fn script(&mut self, file: &str) {
        self.vm.vm.current_file = PhpStr::from(file);
    }

    fn flush(&mut self) {
        let out = self.vm.output();
        self.text.push_str(&out);
    }

    fn echo(&mut self, s: &str) {
        self.flush();
        self.text.push_str(s);
    }

    fn dump(&mut self, v: &Value) {
        self.flush();
        var_dump(v, 0, &mut self.text);
    }

    fn call(&mut self, name: &str, args: Vec<Value>) -> Value {
        self.vm
            .call(name, args)
            .unwrap_or_else(|e| panic!("{name}() threw {e:?}"))
    }

    /// `getimagesize($file)` for a file of the test directory.
    fn gis(&mut self, file: &str) -> Value {
        self.call("getimagesize", vec![Value::from(path(file))])
    }

    /// `getimagesize($file, $info)`; returns the result and `$info`.
    fn gis_info(&mut self, file: &str) -> (Value, Value) {
        let info = PhpRef::new(Value::Null);
        let r = self.call(
            "getimagesize",
            vec![Value::from(path(file)), Value::Ref(info.clone())],
        );
        (r, info.get())
    }

    /// Compares the accumulated output with the `.phpt` expectation.
    fn check(mut self, phpt: &str) {
        self.flush();
        let (expected, is_pattern) = phpt_expect(phpt);
        let got = self.text.trim().to_string();
        if is_pattern {
            assert!(
                expectf_matches(&expected, &got),
                "{phpt}: output does not match\n--- expected ---\n{expected}\n--- got ---\n{got}"
            );
        } else {
            assert_eq!(got, expected, "{phpt}");
        }
    }
}

fn int(v: i64) -> Value {
    Value::Int(v)
}

fn get(v: &Value, key: &str) -> Option<Value> {
    match v {
        Value::Array(a) => a.get(&ArrayKey::from(key)).map(|v| v.unref()),
        _ => None,
    }
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    get(v, key).and_then(|v| v.as_str().map(|s| s.to_string_lossy().into_owned()))
}

fn idx(v: &Value, i: i64) -> Value {
    match v {
        Value::Array(a) => a.get(&ArrayKey::Int(i)).expect("index").unref(),
        other => panic!("not an array: {other:?}"),
    }
}

fn keys(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .keys()
            .map(|k| match k {
                ArrayKey::Int(i) => i.to_string(),
                ArrayKey::Str(s) => s.to_string_lossy().into_owned(),
            })
            .collect(),
        other => panic!("not an array: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// getimagesize*.phpt
// ---------------------------------------------------------------------------

#[test]
fn getimagesize_basic() {
    let files = [
        ("GIF image file", "200x100.gif"),
        ("JPEG image file", "200x100.jpg"),
        ("PNG image file", "200x100.png"),
        ("SWF image file", "200x100.swf"),
        ("BMP image file", "200x100.bmp"),
        ("TIFF intel byte order image file", "200x100.tiff"),
        ("JPC image file", "test1pix.jpc"),
        ("JP2 image file", "test1pix.jp2"),
        ("IFF image file", "test4pix.iff"),
    ];
    let mut run = Run::new();
    run.echo("*** Testing getimagesize() : basic functionality ***\n");
    for (key, filename) in files {
        run.echo(&format!("\n-- {key} ({filename}) --\n"));
        let (r, info) = run.gis_info(filename);
        run.dump(&r);
        run.dump(&info);
    }
    run.check("getimagesize_basic");
}

/// The scripts that glob the directory with `preg_match('/^.../', $file)`.
fn dir_listing(accept: impl Fn(&str) -> bool) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(DIR)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|f| accept(f))
        .collect();
    files.sort();
    files
}

fn getimagesize_dir_test(phpt: &str, accept: impl Fn(&str) -> bool) {
    let mut run = Run::new();
    let mut result = php_runtime::PhpArray::new();
    for file in dir_listing(accept) {
        let r = run.gis(&file);
        result.insert(ArrayKey::from(file.as_str()), r);
    }
    run.dump(&Value::array(result));
    run.check(phpt);
}

/// `/^test.+pix\./` without test13pix.swf.
fn is_test_pix(f: &str) -> bool {
    f.starts_with("test") && f != "test13pix.swf" && f[4..].find("pix.").is_some_and(|i| i >= 1)
}

#[test]
fn getimagesize_all_samples() {
    getimagesize_dir_test("getimagesize", is_test_pix);
}

#[test]
fn getimagesize_246x247() {
    getimagesize_dir_test("getimagesize_246x247", |f| f.starts_with("246x247."));
}

#[test]
fn getimagesize_384x385() {
    getimagesize_dir_test("getimagesize_384x385", |f| f.starts_with("384x385."));
}

#[test]
fn image_type_to_mime_type_of_samples() {
    let mut run = Run::new();
    let mut result = php_runtime::PhpArray::new();
    for file in dir_listing(is_test_pix) {
        let r = run.gis(&file);
        let mime = run.call("image_type_to_mime_type", vec![idx(&r, 2)]);
        result.insert(ArrayKey::from(file.as_str()), mime);
    }
    run.dump(&Value::array(result));
    run.check("image_type_to_mime_type");
}

#[test]
fn getimagesize_jpgapp() {
    let info = PhpRef::new(Value::Null);
    // The array passed in is replaced, not merged into.
    let mut arr = php_runtime::PhpArray::new();
    arr.insert_str("this", "will");
    arr.insert_str("all", "be destroyed!");
    arr.insert_str("APP1", "and this too");
    info.set(Value::array(arr));
    let mut run = Run::new();
    run.call(
        "getimagesize",
        vec![Value::from(path("testAPP.jpg")), Value::Ref(info.clone())],
    );
    let Value::Array(a) = info.get() else {
        panic!("not an array")
    };
    let mut text = String::new();
    for (k, v) in a.iter() {
        let ArrayKey::Str(k) = k else { panic!() };
        let v = v.unref();
        let bytes = v.as_str().unwrap().as_bytes().to_vec();
        text.push_str(&format!(
            "{} - length: {}; md5: {}\n",
            k.to_string_lossy(),
            bytes.len(),
            php_hash::md5_hex(&bytes)
        ));
    }
    run.text = text;
    run.check("getimagesize_jpgapp");
}

#[test]
fn getimagesize_swc() {
    let mut run = Run::new();
    let r = run.gis("test13pix.swf");
    run.dump(&r);
    run.check("getimagesize_swc");

    for phpt in ["getimagesize_variation4", "getimagesize_variation_005"] {
        let mut run = Run::new();
        run.echo(if phpt == "getimagesize_variation4" {
            "*** Testing getimagesize() : variation ***\n"
        } else {
            "*** Testing getimagesize() : basic functionality ***\n"
        });
        let (r, info) = run.gis_info("test13pix.swf");
        run.dump(&r);
        run.dump(&info);
        run.check(phpt);
    }
}

#[test]
fn getimagesize_tif_mm() {
    let mut run = Run::new();
    run.echo("*** Testing getimagesize() : tiff_mm format ***\n");
    let (r, info) = run.gis_info("2x2mm.tiff");
    run.dump(&r);
    run.dump(&info);
    run.check("getimagesize_tif_mm");
}

#[test]
fn getimagesize_wbmp_and_xbm() {
    let mut run = Run::new();
    run.echo("*** Testing getimagesize() : wbmp format ***\n");
    let (r, info) = run.gis_info("75x50.wbmp");
    run.dump(&r);
    run.dump(&info);
    run.check("getimagesize_wbmp");

    let mut run = Run::new();
    run.echo("*** Testing getimagesize() : xbm format ***\n");
    let (r, info) = run.gis_info("75x50.xbm");
    run.dump(&r);
    run.dump(&info);
    run.check("getimagesize_xbm");
}

#[test]
fn getimagesize_256_ico() {
    let mut run = Run::new();
    run.echo("*** Testing getimagesize() : 256px ico ***\n");
    let r = run.gis("32x256.ico");
    run.dump(&r);
    run.echo("===DONE===\n");
    run.check("getimagesize_256_ico");
}

#[test]
fn getimagesize_variation2_non_array_info() {
    let initial = [
        int(0),
        int(12345),
        Value::Float(10.5),
        Value::empty_array(),
        Value::Null,
        Value::Bool(true),
        Value::str(""),
        Value::str("text"),
    ];
    for value in initial {
        let mut run = Run::new();
        let info = PhpRef::new(value);
        run.call(
            "getimagesize",
            vec![Value::from(path("test1pix.jpg")), Value::Ref(info.clone())],
        );
        let app0 = get(&info.get(), "APP0").expect("APP0");
        assert_eq!(
            php_hash::to_hex(app0.as_str().unwrap().as_bytes()),
            "4a46494600010201006000600000"
        );
    }
}

#[test]
fn getimagesize_variation3_non_images() {
    let files = [
        ("File with text data", "test.txt"),
        ("File with corrupted BMP data", "200x100_unknown.unknown"),
        ("Non-existent file", "nofile.ext"),
        ("Empty File", "blank_file.bmp"),
    ];
    let mut run = Run::new();
    run.echo("*** Testing getimagesize() : variation ***\n");
    for (key, filename) in files {
        run.echo(&format!("\n-- {key} ({filename}) --\n"));
        let r = run.gis(filename);
        run.dump(&r);
        let (r, info) = run.gis_info(filename);
        run.dump(&r);
        run.dump(&info);
    }
    run.check("getimagesize_variation3");
}

#[test]
fn getimagesizefromstring_matches_getimagesize() {
    let mut run = Run::new();
    let i1 = run.gis("test.gif");
    let data = read("test.gif");
    let i2 = run.call("getimagesizefromstring", vec![Value::from(data)]);
    run.dump(&i1);
    run.dump(&i2);
    run.check("getimagesizefromstring1");
}

#[test]
fn getimagesizefromstring_iff_overflow() {
    let payload = b"FORM\x00\x00\x00\x00ILBMABCD\x7f\xff\xff\xff".to_vec();
    let mut run = Run::new();
    let r = run.call("getimagesizefromstring", vec![Value::from(payload.clone())]);
    run.dump(&r);
    let file = std::env::temp_dir().join(format!("php_image_iff_{}.iff", std::process::id()));
    std::fs::write(&file, &payload).unwrap();
    let r = run.call(
        "getimagesize",
        vec![Value::from(file.to_string_lossy().into_owned())],
    );
    let _ = std::fs::remove_file(&file);
    run.dump(&r);
    run.check("getimagesizefromstring_iff_overflow");
}

#[test]
fn bug13213_and_bug72278_corrupt_jpeg_warning() {
    for (phpt, file) in [("bug13213", "bug13213.jpg"), ("bug72278", "bug72278.jpg")] {
        let mut run = Run::new();
        run.script(&format!("{DIR}/{phpt}.php"));
        let r = run.gis(file);
        run.dump(&r);
        run.check(phpt);
    }
}

#[test]
fn bug70052_wbmp_sizes() {
    let mut run = Run::new();
    let r = run.gis("bug70052_1.wbmp");
    run.dump(&r);
    let r = run.gis("bug70052_2.wbmp");
    run.dump(&r);
    run.check("bug70052");
}

#[test]
fn bug71848_jpeg_with_app5() {
    let mut run = Run::new();
    let (r, info) = run.gis_info("bug71848.jpg");
    run.dump(&r);
    let k = Value::array(php_runtime::PhpArray::from_values(
        keys(&info).into_iter().map(Value::from).collect(),
    ));
    run.dump(&k);
    run.check("bug71848");
}

#[test]
fn gh20201_avif_is_not_heif() {
    let mut run = Run::new();
    let r = run.gis("gh20201.avif");
    run.dump(&r);
    run.check("gh20201");
}

#[test]
fn bug79877_null_bytes() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call("getimagesize", vec![Value::str("/tmp/a.png\0xx")]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "getimagesize(): Argument #1 ($filename) must not contain any null bytes"
    );
    // getimagesizefromstring() accepts NUL bytes (they are image data).
    let r = t
        .call("getimagesizefromstring", vec![Value::str("a\0b")])
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
}

#[test]
fn empty_path_is_a_value_error() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call("getimagesize", vec![Value::str("")]));
    assert_eq!(class, "ValueError");
    assert_eq!(msg, "Path must not be empty");
}

// ---------------------------------------------------------------------------
// image_type_to_*.phpt
// ---------------------------------------------------------------------------

#[test]
fn image_type_to_extension() {
    const NAMES: [(&str, i64); 19] = [
        ("IMAGETYPE_GIF", 0),
        ("IMAGETYPE_JPEG", 0),
        ("IMAGETYPE_PNG", 0),
        ("IMAGETYPE_SWF", 0),
        ("IMAGETYPE_PSD", 0),
        ("IMAGETYPE_BMP", 0),
        ("IMAGETYPE_TIFF_II", 0),
        ("IMAGETYPE_TIFF_MM", 0),
        ("IMAGETYPE_JPC", 0),
        ("IMAGETYPE_JP2", 0),
        ("IMAGETYPE_JPX", 0),
        ("IMAGETYPE_JB2", 0),
        ("IMAGETYPE_IFF", 0),
        ("IMAGETYPE_WBMP", 0),
        ("IMAGETYPE_JPEG2000", 0),
        ("IMAGETYPE_XBM", 0),
        ("IMAGETYPE_WEBP", 0),
        ("IMAGETYPE_AVIF", 0),
        ("IMAGETYPE_HEIF", 0),
    ];
    let mut run = Run::new();
    for (name, _) in NAMES {
        let c = run.vm.vm.constant(name).unwrap_or_else(|| panic!("{name}"));
        let with = run.call("image_type_to_extension", vec![c.clone()]);
        let without = run.call("image_type_to_extension", vec![c, Value::Bool(false)]);
        run.echo(&format!(
            "Constant: {name}\n\tWith dot: {}\n\tWithout dot: {}\n",
            with.as_str().unwrap().to_string_lossy(),
            without.as_str().unwrap().to_string_lossy()
        ));
    }
    let r = run.call(
        "image_type_to_extension",
        vec![int(1000000), Value::Bool(false)],
    );
    run.dump(&r);
    let r = run.call("image_type_to_extension", vec![int(0)]);
    run.dump(&r);
    run.echo("Done\n");
    run.check("image_type_to_extension");
}

#[test]
fn image_type_to_mime_type_basic() {
    let names = [
        "IMAGETYPE_GIF",
        "IMAGETYPE_JPEG",
        "IMAGETYPE_PNG",
        "IMAGETYPE_SWF",
        "IMAGETYPE_PSD",
        "IMAGETYPE_BMP",
        "IMAGETYPE_TIFF_II",
        "IMAGETYPE_TIFF_MM",
        "IMAGETYPE_JPC",
        "IMAGETYPE_JP2",
        "IMAGETYPE_JPX",
        "IMAGETYPE_JB2",
        "IMAGETYPE_IFF",
        "IMAGETYPE_WBMP",
        "IMAGETYPE_JPEG2000",
        "IMAGETYPE_HEIF",
        "IMAGETYPE_XBM",
        "IMAGETYPE_WEBP",
        "IMAGETYPE_HEIF",
    ];
    let mut run = Run::new();
    run.echo("Starting image_type_to_mime_type() test\n\n");
    for name in names {
        let c = run.vm.vm.constant(name).unwrap();
        let r = run.call("image_type_to_mime_type", vec![c]);
        run.dump(&r);
    }
    run.echo("\nDone image_type_to_mime_type() test\n");
    run.check("image_type_to_mime_type_basic");
}

#[test]
fn image_type_to_mime_type_variation2() {
    let values: [i64; 12] = [
        0, 1, 12345, -12345, 0o2, 0o10, 0o30071, -0o30071, 0x0, 0x1, 0xABCD, -0xABCD,
    ];
    let mut run = Run::new();
    run.echo("*** Testing image_type_to_mime_type() : usage variations ***\n");
    for (i, v) in values.iter().enumerate() {
        run.echo(&format!("\n-- Iteration {} --\n", i + 1));
        let r = run.call("image_type_to_mime_type", vec![int(*v)]);
        run.dump(&r);
    }
    run.check("image_type_to_mime_type_variation2");
}

#[test]
fn image_type_to_mime_type_variation3_and_4() {
    let mut run = Run::new();
    run.echo("*** Testing image_type_to_mime_type() : usage variations ***\n");
    for t in 0..=20 {
        run.echo(&format!("\n-- Iteration {t} --\n"));
        let r = run.call("image_type_to_mime_type", vec![int(t)]);
        run.dump(&r);
    }
    run.echo("\n-- Iteration 999 --\n");
    let r = run.call("image_type_to_mime_type", vec![int(999)]);
    run.dump(&r);
    run.check("image_type_to_mime_type_variation3");

    let mut run = Run::new();
    run.echo("*** Testing image_type_to_mime_type() : usage variations ***\n");
    for name in ["IMAGETYPE_ICO", "IMAGETYPE_SWC"] {
        let c = run.vm.vm.constant(name).unwrap();
        let r = run.call("image_type_to_mime_type", vec![c]);
        run.dump(&r);
    }
    run.check("image_type_to_mime_type_variation4");
}

#[test]
fn image_type_to_mime_type_truncates_to_int() {
    // php_image_type_to_mime_type() takes a C int.
    let mut t = TestVm::new();
    let r = t
        .call("image_type_to_mime_type", vec![int((1 << 32) + 2)])
        .unwrap();
    assert_eq!(r.as_str().unwrap(), "image/jpeg");
    // image_type_to_extension() switches on the full zend_long.
    let r = t
        .call("image_type_to_extension", vec![int((1 << 32) + 2)])
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
}

#[test]
fn constants_have_php_values() {
    let t = TestVm::new();
    let expect = [
        ("IMAGETYPE_GIF", 1),
        ("IMAGETYPE_JPEG", 2),
        ("IMAGETYPE_PNG", 3),
        ("IMAGETYPE_SWF", 4),
        ("IMAGETYPE_PSD", 5),
        ("IMAGETYPE_BMP", 6),
        ("IMAGETYPE_TIFF_II", 7),
        ("IMAGETYPE_TIFF_MM", 8),
        ("IMAGETYPE_JPC", 9),
        ("IMAGETYPE_JP2", 10),
        ("IMAGETYPE_JPX", 11),
        ("IMAGETYPE_JB2", 12),
        ("IMAGETYPE_SWC", 13),
        ("IMAGETYPE_IFF", 14),
        ("IMAGETYPE_WBMP", 15),
        ("IMAGETYPE_JPEG2000", 9),
        ("IMAGETYPE_XBM", 16),
        ("IMAGETYPE_ICO", 17),
        ("IMAGETYPE_WEBP", 18),
        ("IMAGETYPE_AVIF", 19),
        ("IMAGETYPE_HEIF", 20),
        ("IMAGETYPE_UNKNOWN", 0),
        ("IMAGETYPE_COUNT", 21),
    ];
    for (name, value) in expect {
        let c = t.vm.constant(name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(c.as_int(), Some(value), "{name}");
    }
}

// ---------------------------------------------------------------------------
// Hand-written edge cases (php-src behaviour read from image.c)
// ---------------------------------------------------------------------------

fn from_string(t: &mut TestVm, data: &[u8]) -> Value {
    t.call("getimagesizefromstring", vec![Value::from(data.to_vec())])
        .unwrap()
}

fn flat(v: &Value) -> String {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|(k, v)| {
                let k = match k {
                    ArrayKey::Int(i) => i.to_string(),
                    ArrayKey::Str(s) => s.to_string_lossy().into_owned(),
                };
                format!("{k}={}", flat(&v.unref()))
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Int(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.to_string_lossy().into_owned(),
        other => format!("{other:?}"),
    }
}

#[test]
fn short_input_reports_error_reading() {
    let mut t = TestVm::new();
    let r = from_string(&mut t, b"");
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nNotice: getimagesizefromstring(): Error reading from ! in /t.php on line 3\n"
    );
    // %s stops at a NUL byte of the input.
    let r = from_string(&mut t, b"ab");
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nNotice: getimagesizefromstring(): Error reading from ab! in /t.php on line 3\n"
    );
    let _ = from_string(&mut t, b"\x89PNG");
    assert_eq!(
        t.output(),
        "\nNotice: getimagesizefromstring(): Error reading from \u{fffd}PNG! in /t.php on line 3\n"
    );
}

#[test]
fn corrupted_png_signature_warns() {
    let mut t = TestVm::new();
    let r = from_string(&mut t, b"\x89PNG\r\n\x1a\x0b\x00\x00\x00\x0dIHDR");
    assert!(matches!(r, Value::Bool(false)));
    assert!(t.output().contains(
        "Warning: getimagesizefromstring(): PNG file corrupted by ASCII conversion in /t.php on line 3"
    ));
}

#[test]
fn gif_png_psd_bmp_from_memory() {
    let mut t = TestVm::new();
    let gif = [b"GIF89a".as_slice(), &[3, 0, 2, 0, 0xf2, 0, 0]].concat();
    assert_eq!(
        flat(&from_string(&mut t, &gif)),
        "0=3,1=2,2=1,3=width=\"3\" height=\"2\",bits=3,channels=3,mime=image/gif,width_unit=px,height_unit=px"
    );
    // A GIF without a global colour table has no "bits".
    let gif = [b"GIF89a".as_slice(), &[3, 0, 2, 0, 0x00, 0, 0]].concat();
    assert_eq!(
        flat(&from_string(&mut t, &gif)),
        "0=3,1=2,2=1,3=width=\"3\" height=\"2\",channels=3,mime=image/gif,width_unit=px,height_unit=px"
    );
    let mut psd = b"8BPS".to_vec();
    psd.extend_from_slice(&[0; 10]); // version, reserved, channels
    psd.extend_from_slice(&7u32.to_be_bytes()); // height
    psd.extend_from_slice(&9u32.to_be_bytes()); // width
    assert_eq!(
        flat(&from_string(&mut t, &psd)),
        "0=9,1=7,2=5,3=width=\"9\" height=\"7\",mime=image/psd,width_unit=px,height_unit=px"
    );
    // OS/2 BMP header (12 bytes): 16-bit dimensions; php reads "bits" from
    // byte 11 of the 16 bytes it fetches (the high byte of the bit count).
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&[0; 12]);
    bmp.extend_from_slice(&12u32.to_le_bytes());
    bmp.extend_from_slice(&[5, 0, 6, 0, 1, 0, 0, 24, 0, 0, 0, 0]);
    assert_eq!(
        flat(&from_string(&mut t, &bmp)),
        "0=5,1=6,2=6,3=width=\"5\" height=\"6\",bits=24,mime=image/bmp,width_unit=px,height_unit=px"
    );
    // Top-down BMP: negative height is reported as its absolute value.
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&[0; 12]);
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&10u32.to_le_bytes());
    bmp.extend_from_slice(&(-20i32).to_le_bytes());
    bmp.extend_from_slice(&[1, 0, 24, 0]);
    assert_eq!(
        flat(&from_string(&mut t, &bmp)),
        "0=10,1=20,2=6,3=width=\"10\" height=\"20\",bits=24,mime=image/bmp,width_unit=px,height_unit=px"
    );
    assert_eq!(t.output(), "");
}

#[test]
fn huge_dimensions_wrap_in_the_attribute_string() {
    // The `width="%d"` string prints the unsigned value as a signed int.
    let mut t = TestVm::new();
    let mut psd = b"8BPS".to_vec();
    psd.extend_from_slice(&[0; 10]);
    psd.extend_from_slice(&1u32.to_be_bytes());
    psd.extend_from_slice(&0xffff_fffeu32.to_be_bytes());
    assert_eq!(
        flat(&from_string(&mut t, &psd)),
        "0=4294967294,1=1,2=5,3=width=\"-2\" height=\"1\",mime=image/psd,width_unit=px,height_unit=px"
    );
}

#[test]
fn jpeg_markers_and_info() {
    let mut t = TestVm::new();
    // SOI, APP1 "Exif", a second APP1 (ignored), COM, SOF0 (3x2, 8 bit, 3 channels), SOS.
    let mut jpeg = vec![0xff, 0xd8];
    jpeg.extend_from_slice(&[0xff, 0xe1, 0x00, 0x06, b'E', b'x', b'i', b'f']);
    jpeg.extend_from_slice(&[0xff, 0xe1, 0x00, 0x04, b'x', b'y']);
    jpeg.extend_from_slice(&[0xff, 0xfe, 0x00, 0x04, b'h', b'i']);
    jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 8, 0, 2, 0, 3, 3]);
    jpeg.extend_from_slice(&[1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    jpeg.extend_from_slice(&[0xff, 0xda, 0x00, 0x02]);
    let info = PhpRef::new(Value::Null);
    let r = t
        .call(
            "getimagesizefromstring",
            vec![Value::from(jpeg.clone()), Value::Ref(info.clone())],
        )
        .unwrap();
    assert_eq!(
        flat(&r),
        "0=3,1=2,2=2,3=width=\"3\" height=\"2\",bits=8,channels=3,mime=image/jpeg,width_unit=px,height_unit=px"
    );
    // Only the first block of each APPn kind is stored.
    assert_eq!(flat(&info.get()), "APP1=Exif");
    // Without $image_info the APP blocks are skipped.
    assert_eq!(flat(&from_string(&mut t, &jpeg)), flat(&r));
    assert_eq!(t.output(), "");
}

#[test]
fn jpeg_extraneous_bytes_and_truncation() {
    let mut t = TestVm::new();
    // An empty APP0, then garbage before the next marker -> warning with the
    // number of bytes skipped (padding 0xff bytes are swallowed silently).
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x02];
    jpeg.extend_from_slice(&[1, 2, 3, 0xff, 0xff, 0xc0, 0x00, 0x0b, 8, 0, 1, 0, 1, 1]);
    let r = from_string(&mut t, &jpeg);
    assert_eq!(
        flat(&r),
        "0=1,1=1,2=2,3=width=\"1\" height=\"1\",bits=8,channels=1,mime=image/jpeg,width_unit=px,height_unit=px"
    );
    assert_eq!(
        t.output(),
        "\nWarning: getimagesizefromstring(): Corrupt JPEG data: 3 extraneous bytes before marker in /t.php on line 3\n"
    );
    // A JPEG that ends before any SOF block: no result.
    let r = from_string(&mut t, &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0, 0]);
    assert!(matches!(r, Value::Bool(false)));
}

#[test]
fn jpeg_truncated_sof_reports_eof_bits() {
    // getc() returns EOF (-1) which is stored in an unsigned int.
    let mut t = TestVm::new();
    let r = from_string(&mut t, &[0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b]);
    assert_eq!(get(&r, "bits").and_then(|v| v.as_int()), Some(4294967295));
    assert_eq!(
        get(&r, "channels").and_then(|v| v.as_int()),
        Some(4294967295)
    );
}

#[test]
fn ico_picks_the_highest_bit_depth() {
    let mut t = TestVm::new();
    let mut ico = vec![0, 0, 1, 0, 2, 0];
    // 16x16 4 bit, then 0x0 (=256x256) 32 bit.
    ico.extend_from_slice(&[16, 16, 0, 0, 1, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    ico.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(
        flat(&from_string(&mut t, &ico)),
        "0=256,1=256,2=17,3=width=\"256\" height=\"256\",bits=32,mime=image/vnd.microsoft.icon,width_unit=px,height_unit=px"
    );
    // Zero icons: invalid.
    assert!(matches!(
        from_string(&mut t, &[0, 0, 1, 0, 0, 0]),
        Value::Bool(false)
    ));
}

#[test]
fn webp_variants() {
    let mut t = TestVm::new();
    let riff = |chunk: &[u8]| {
        let mut v = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        v.extend_from_slice(chunk);
        v
    };
    // VP8X: 24-bit minus-one dimensions at offsets 12.. of the chunk.
    let mut c = b"VP8X\x0a\x00\x00\x00\x00\x00\x00\x00".to_vec();
    c.extend_from_slice(&[0x31, 0x00, 0x00, 0x1f, 0x00, 0x00]);
    assert_eq!(
        flat(&from_string(&mut t, &riff(&c))),
        "0=50,1=32,2=18,3=width=\"50\" height=\"32\",bits=8,mime=image/webp,width_unit=px,height_unit=px"
    );
    // VP8L.
    let mut c = b"VP8L\x00\x00\x00\x00\x2f".to_vec();
    // 14 bits width-1 = 3, 14 bits height-1 = 2 -> bytes: 03 80 00 00? (little endian bit packing)
    c.extend_from_slice(&[0x03, 0x80, 0x00, 0x00, 0, 0, 0, 0, 0]);
    let r = from_string(&mut t, &riff(&c));
    assert_eq!(get(&r, "bits").and_then(|v| v.as_int()), Some(8));
    assert_eq!(idx(&r, 0).as_int(), Some(4));
    assert_eq!(idx(&r, 1).as_int(), Some(3));
    // Unknown chunk and RIFF that is not WebP.
    assert!(matches!(
        from_string(&mut t, &riff(b"VP8Zxxxxxxxxxxxxxx")),
        Value::Bool(false)
    ));
    assert!(matches!(
        from_string(&mut t, b"RIFF\x00\x00\x00\x00WAVEfmt "),
        Value::Bool(false)
    ));
    assert_eq!(t.output(), "");
}

#[test]
fn tiff_both_byte_orders() {
    let mut t = TestVm::new();
    // Little endian: IFD right after the header with width/height as SHORT.
    let mut le = vec![b'I', b'I', 0x2a, 0, 8, 0, 0, 0, 2, 0];
    le.extend_from_slice(&[0x00, 0x01, 3, 0, 1, 0, 0, 0, 7, 0, 0, 0]);
    le.extend_from_slice(&[0x01, 0x01, 4, 0, 1, 0, 0, 0, 9, 0, 0, 0]);
    le.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(
        flat(&from_string(&mut t, &le)),
        "0=7,1=9,2=7,3=width=\"7\" height=\"9\",mime=image/tiff,width_unit=px,height_unit=px"
    );
    let mut be = vec![b'M', b'M', 0, 0x2a, 0, 0, 0, 8, 0, 2];
    be.extend_from_slice(&[0x01, 0x00, 0, 3, 0, 0, 0, 1, 0, 7, 0, 0]);
    be.extend_from_slice(&[0x01, 0x01, 0, 4, 0, 0, 0, 1, 0, 0, 0, 9]);
    be.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(
        flat(&from_string(&mut t, &be)),
        "0=7,1=9,2=8,3=width=\"7\" height=\"9\",mime=image/tiff,width_unit=px,height_unit=px"
    );
    // No width/height tags: no result.
    let none = vec![b'I', b'I', 0x2a, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert!(matches!(from_string(&mut t, &none), Value::Bool(false)));
}

#[test]
fn iff_bmhd_chunk() {
    let mut t = TestVm::new();
    let mut iff = b"FORM\x00\x00\x00\x20ILBM".to_vec();
    iff.extend_from_slice(b"CMAP\x00\x00\x00\x03abc\x00"); // odd size is padded
    iff.extend_from_slice(b"BMHD\x00\x00\x00\x14");
    iff.extend_from_slice(&[0, 5, 0, 6, 0, 0, 0, 0, 8, 0, 0, 0]);
    assert_eq!(
        flat(&from_string(&mut t, &iff)),
        "0=5,1=6,2=14,3=width=\"5\" height=\"6\",bits=8,mime=image/iff,width_unit=px,height_unit=px"
    );
    // Other signature after FORM: not IFF-ILBM.
    assert!(matches!(
        from_string(&mut t, b"FORM\x00\x00\x00\x00WAVE12345678"),
        Value::Bool(false)
    ));
}

#[test]
fn wbmp_and_xbm_detection() {
    let mut t = TestVm::new();
    // type 0, fixed header 0, width 300 (multi-byte), height 5.
    let wbmp = [0u8, 0, 0x82, 0x2c, 5];
    assert_eq!(
        flat(&from_string(&mut t, &wbmp)),
        "0=300,1=5,2=15,3=width=\"300\" height=\"5\",mime=image/vnd.wap.wbmp,width_unit=px,height_unit=px"
    );
    // Dimensions above 2048 are rejected; the short non-image then reports a
    // read error (the "input" is printed as a C string, here empty).
    assert!(matches!(
        from_string(&mut t, &[0u8, 0, 0xff, 0x7f, 5]),
        Value::Bool(false)
    ));
    assert_eq!(
        t.output(),
        "\nNotice: getimagesizefromstring(): Error reading from ! in /t.php on line 3\n"
    );
    let xbm =
        b"#define test_width 16\n#define test_height 9\nstatic char test_bits[] = {\n 0x00 };\n";
    assert_eq!(
        flat(&from_string(&mut t, xbm)),
        "0=16,1=9,2=16,3=width=\"16\" height=\"9\",mime=image/xbm,width_unit=px,height_unit=px"
    );
    // The name may have no underscore; extra whitespace is fine.
    let xbm = b"#define   width   16\n\n#define\theight\t9\n";
    assert_eq!(idx(&from_string(&mut t, xbm), 0).as_int(), Some(16));
    // Needs both dimensions.
    assert!(matches!(
        from_string(&mut t, b"#define a_width 16\nxxxxxxxxxxxxxxxxxxx"),
        Value::Bool(false)
    ));
    assert_eq!(t.output(), "");
}

#[test]
fn jpc_and_jp2() {
    let mut t = TestVm::new();
    // A codestream header: SOC, SIZ, Xsiz=3, Ysiz=2, 2 components of 8 and 12 bit.
    let mut jpc = vec![0xff, 0x4f, 0xff, 0x51, 0, 0x2f, 0, 0];
    jpc.extend_from_slice(&3u32.to_be_bytes());
    jpc.extend_from_slice(&2u32.to_be_bytes());
    jpc.extend_from_slice(&[0; 24]);
    jpc.extend_from_slice(&[0, 2, 7, 1, 1, 11, 1, 1]);
    assert_eq!(
        flat(&from_string(&mut t, &jpc)),
        "0=3,1=2,2=9,3=width=\"3\" height=\"2\",bits=12,channels=2,mime=application/octet-stream,width_unit=px,height_unit=px"
    );
    // Missing SIZ marker.
    let r = from_string(&mut t, &[0xff, 0x4f, 0xff, 0x52, 0, 0]);
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nWarning: getimagesizefromstring(): JPEG2000 codestream corrupt(Expected SIZ marker not found after SOC) in /t.php on line 3\n"
    );
    // JP2 without a codestream box.
    let mut jp2 = vec![
        0, 0, 0, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a,
    ];
    jp2.extend_from_slice(&[0, 0, 0, 0x14, b'f', b't', b'y', b'p']);
    jp2.extend_from_slice(&[0; 12]);
    let r = from_string(&mut t, &jp2);
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nWarning: getimagesizefromstring(): JP2 file has no codestreams at root level in /t.php on line 3\n"
    );
    // JP2 with a jp2c box wrapping the codestream.
    let mut jp2 = vec![
        0, 0, 0, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a,
    ];
    jp2.extend_from_slice(&[0, 0, 0, 0x30, b'j', b'p', b'2', b'c']);
    jp2.extend_from_slice(&jpc);
    let r = from_string(&mut t, &jp2);
    assert_eq!(idx(&r, 2).as_int(), Some(10));
    assert_eq!(idx(&r, 0).as_int(), Some(3));
    assert_eq!(get_str(&r, "mime").as_deref(), Some("image/jp2"));
}

#[test]
fn heif_sample() {
    let mut run = Run::new();
    let r = run.gis("test4pix.heic");
    assert_eq!(idx(&r, 2).as_int(), Some(20));
    assert_eq!(get_str(&r, "mime").as_deref(), Some("image/heif"));
    assert_eq!(idx(&r, 0).as_int(), Some(54));
    assert_eq!(idx(&r, 1).as_int(), Some(84));
    assert_eq!(get(&r, "bits").and_then(|v| v.as_int()), Some(8));
    assert_eq!(get(&r, "channels").and_then(|v| v.as_int()), Some(3));
}

#[test]
fn cws_with_stored_deflate_block() {
    // A compressed SWF whose zlib stream is shorter than the 64 bytes read
    // first: the first uncompress() attempt succeeds.
    let mut header = vec![0u8; 0];
    // RECT: 15 bits per field, xmin=0, xmax=11000 (550px), ymin=0, ymax=8000 (400px).
    let mut bits = String::new();
    for v in [15u32, 0, 11000, 0, 8000] {
        let width = if v == 15 && bits.is_empty() { 5 } else { 15 };
        bits.push_str(&format!("{:0width$b}", v, width = width));
    }
    #[allow(clippy::manual_is_multiple_of)]
    while bits.len() % 8 != 0 {
        bits.push('0');
    }
    for chunk in bits.as_bytes().chunks(8) {
        header.push(u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 2).unwrap());
    }
    let mut z = vec![0x78, 0x01, 0x01];
    z.extend_from_slice(&(header.len() as u16).to_le_bytes());
    z.extend_from_slice(&(!(header.len() as u16)).to_le_bytes());
    z.extend_from_slice(&header);
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &header {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&(b << 16 | a).to_be_bytes());
    let mut swf = b"CWS\x06\x00\x00\x00\x00".to_vec();
    swf.extend_from_slice(&z);
    // The file must hold the 64 bytes the C code reads.
    swf.resize(8 + 64, 0);
    let mut t = TestVm::new();
    assert_eq!(
        flat(&from_string(&mut t, &swf)),
        "0=550,1=400,2=13,3=width=\"550\" height=\"400\",mime=application/x-shockwave-flash,width_unit=px,height_unit=px"
    );
    // Truncated below 72 bytes: the 64-byte read fails.
    assert!(matches!(
        from_string(&mut t, &swf[..60]),
        Value::Bool(false)
    ));
    // Garbage zlib data: both attempts fail.
    let mut bad = b"CWS\x06\x00\x00\x00\x00".to_vec();
    bad.extend_from_slice(&[0xffu8; 70]);
    assert!(matches!(from_string(&mut t, &bad), Value::Bool(false)));
}

#[test]
fn local_file_open_errors() {
    let mut t = TestVm::new();
    let r = t
        .call("getimagesize", vec![Value::str("/definitely/not/here.png")])
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nWarning: getimagesize(): Failed to open stream: No such file or directory in /t.php on line 3\n"
    );
    // A directory opens, but reading fails.
    let r = t
        .call("getimagesize", vec![Value::from(DIR.to_string())])
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    let out = t.output();
    assert!(
        out.contains(
            "Notice: getimagesize(): Read of 8192 bytes failed with errno=21 Is a directory"
        ),
        "{out}"
    );
    assert!(
        out.contains("Notice: getimagesize(): Error reading from "),
        "{out}"
    );
    // file:// URLs of local files work, remote hosts do not.
    let url = format!("file://{}", path("test1pix.jpg"));
    let r = t.call("getimagesize", vec![Value::from(url)]).unwrap();
    assert_eq!(idx(&r, 2).as_int(), Some(2));
    let r = t
        .call("getimagesize", vec![Value::str("file://example.com/x.png")])
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        "\nWarning: getimagesize(): Remote host file access not supported, file://example.com/x.png in /t.php on line 3\n"
    );
    let url = format!("file://localhost{}", path("test1pix.jpg"));
    let r = t.call("getimagesize", vec![Value::from(url)]).unwrap();
    assert_eq!(idx(&r, 0).as_int(), Some(1));
}

#[test]
fn info_is_reset_even_on_failure() {
    let mut t = TestVm::new();
    let info = PhpRef::new(Value::Int(5));
    let r = t
        .call(
            "getimagesize",
            vec![
                Value::str("/definitely/not/here.png"),
                Value::Ref(info.clone()),
            ],
        )
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    t.output();
    assert!(matches!(info.get(), Value::Array(a) if a.is_empty()));
}
