//! Ports of php-src's url.c tests (`ext/standard/tests/url`, `strings/url_t.phpt`).
//! The scripts are replayed in Rust and compared with the `--EXPECT--`
//! sections of the vendored `.phpt` files; the URL lists come from the
//! vendored `urls.inc` and test sources themselves.

use php_runtime::value::float_fmt;
use php_runtime::{ArrayKey, PResult, Value};

use super::*;
use crate::testing::{TestVm, thrown};

macro_rules! phpt {
    ($name:literal) => {
        include_str!(concat!(
            "../../../../../tests/phpt/ext/standard/tests/",
            $name
        ))
    };
}

const URLS_INC: &str = phpt!("url/urls.inc");

/// Collects a replayed script's output.
struct Script {
    t: TestVm,
    out: Vec<u8>,
}

impl Script {
    fn new() -> Script {
        Script {
            t: TestVm::new(),
            out: Vec::new(),
        }
    }

    fn try_call(&mut self, f: &str, args: Vec<Value>) -> PResult<Value> {
        let r = self.t.call(f, args);
        let o = self.t.out.take();
        self.out.extend_from_slice(&o);
        r
    }

    fn parse_url(&mut self, url: &[u8], component: Option<i64>) -> Value {
        let mut args = vec![Value::bytes(url)];
        args.extend(component.map(Value::Int));
        match self.try_call("parse_url", args) {
            Ok(v) => v,
            Err(e) => panic!("parse_url failed: {e:?}"),
        }
    }

    fn echo(&mut self, s: impl AsRef<[u8]>) {
        self.out.extend_from_slice(s.as_ref());
    }

    fn var_dump(&mut self, v: &Value) {
        var_dump_into(&mut self.out, v, 0);
    }

    fn check(self, phpt: &str) {
        let expected = expect_section(phpt);
        let actual = String::from_utf8_lossy(&self.out);
        assert_eq!(actual.trim(), expected.trim());
    }
}

/// The `--EXPECT--` section of a .phpt file.
fn expect_section(phpt: &str) -> &str {
    let start = phpt.find("--EXPECT--\n").expect("no EXPECT section") + "--EXPECT--\n".len();
    let rest = &phpt[start..];
    match rest.find("\n--CLEAN--") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

/// The `--FILE--` section of a .phpt file.
fn file_section(phpt: &str) -> &str {
    let start = phpt.find("--FILE--\n").expect("no FILE section");
    let rest = &phpt[start..];
    &rest[..rest.find("\n--EXPECT").expect("no EXPECT section")]
}

/// A PHP single-quoted string literal body.
fn unescape_single(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bytes = s.bytes();
    while let Some(c) = bytes.next() {
        if c == b'\\' {
            match bytes.next() {
                Some(n @ (b'\\' | b'\'')) => out.push(n),
                Some(n) => out.extend_from_slice(&[b'\\', n]),
                None => out.push(b'\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The `'...',` lines of a PHP array literal (urls.inc, url_t.phpt).
fn quoted_lines(src: &str) -> Vec<Vec<u8>> {
    src.lines()
        .map(str::trim)
        .filter(|l| l.len() >= 3 && l.starts_with('\'') && l.ends_with("',"))
        .map(|l| unescape_single(&l[1..l.len() - 2]))
        .collect()
}

fn urls() -> Vec<Vec<u8>> {
    let urls = quoted_lines(URLS_INC);
    assert_eq!(urls.len(), 106, "urls.inc changed");
    urls
}

/// The string literal arguments of every `parse_url(...)` call in a script.
fn parse_url_literals(script: &str) -> Vec<Vec<u8>> {
    let mut found = Vec::new();
    let mut rest = script;
    while let Some(i) = rest.find("parse_url(") {
        rest = &rest[i + "parse_url(".len()..];
        let quote = rest.chars().next().expect("argument");
        assert!(
            quote == '\'' || quote == '"',
            "non-literal parse_url() argument"
        );
        let end = rest[1..].find(quote).expect("unterminated literal") + 1;
        found.push(rest.as_bytes()[1..end].to_vec());
        rest = &rest[end..];
    }
    found
}

fn var_dump_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
    match v {
        Value::Null | Value::Undef => out.extend_from_slice(b"NULL\n"),
        Value::Bool(b) => out.extend_from_slice(format!("bool({b})\n").as_bytes()),
        Value::Int(i) => out.extend_from_slice(format!("int({i})\n").as_bytes()),
        Value::Float(d) => {
            out.extend_from_slice(format!("float({})\n", float_fmt::repr(*d, -1)).as_bytes())
        }
        Value::String(s) => {
            out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
            out.extend_from_slice(s);
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            let pad = " ".repeat(indent + 2);
            for (k, v) in a.iter() {
                out.extend_from_slice(pad.as_bytes());
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
                    ArrayKey::Str(s) => {
                        out.extend_from_slice(b"[\"");
                        out.extend_from_slice(&s);
                        out.extend_from_slice(b"\"]=>\n");
                    }
                }
                out.extend_from_slice(pad.as_bytes());
                var_dump_into(out, v, indent + 2);
            }
            out.extend_from_slice(" ".repeat(indent).as_bytes());
            out.extend_from_slice(b"}\n");
        }
        other => panic!("var_dump of {other:?} not supported in tests"),
    }
}

fn dump(v: &Value) -> String {
    let mut out = Vec::new();
    var_dump_into(&mut out, v, 0);
    String::from_utf8_lossy(&out).into_owned()
}

fn parse(url: &str, component: Option<i64>) -> Value {
    Script::new().parse_url(url.as_bytes(), component)
}

fn call_str(f: &str, arg: &[u8]) -> Vec<u8> {
    let mut t = TestVm::new();
    match t.call(f, vec![Value::bytes(arg)]) {
        Ok(Value::String(s)) => s.to_vec(),
        other => panic!("{f}: unexpected {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// parse_url() .phpt ports
// ---------------------------------------------------------------------------

fn replay_all_components(phpt: &str) {
    let mut s = Script::new();
    for url in urls() {
        s.echo(b"\n--> ");
        s.echo(&url);
        s.echo(b": ");
        let v = s.parse_url(&url, None);
        s.var_dump(&v);
    }
    s.echo("Done");
    s.check(phpt);
}

#[test]
fn parse_url_basic_001_phpt() {
    replay_all_components(phpt!("url/parse_url_basic_001.phpt"));
}

#[test]
fn parse_url_unterminated_phpt() {
    // Same URLs as basic_001; our parser works on slices, never on NUL terminators.
    replay_all_components(phpt!("url/parse_url_unterminated.phpt"));
}

fn replay_component(phpt: &str, component: i64) {
    let mut s = Script::new();
    for url in urls() {
        s.echo(b"--> ");
        s.echo(&url);
        s.echo(b"   : ");
        let v = s.parse_url(&url, Some(component));
        s.var_dump(&v);
    }
    s.echo("Done");
    s.check(phpt);
}

#[test]
fn parse_url_basic_002_to_009_phpt() {
    replay_component(phpt!("url/parse_url_basic_002.phpt"), PHP_URL_SCHEME);
    replay_component(phpt!("url/parse_url_basic_003.phpt"), PHP_URL_HOST);
    replay_component(phpt!("url/parse_url_basic_004.phpt"), PHP_URL_PORT);
    replay_component(phpt!("url/parse_url_basic_005.phpt"), PHP_URL_USER);
    replay_component(phpt!("url/parse_url_basic_006.phpt"), PHP_URL_PASS);
    replay_component(phpt!("url/parse_url_basic_007.phpt"), PHP_URL_PATH);
    replay_component(phpt!("url/parse_url_basic_008.phpt"), PHP_URL_QUERY);
    replay_component(phpt!("url/parse_url_basic_009.phpt"), PHP_URL_FRAGMENT);
}

#[test]
fn parse_url_basic_010_phpt() {
    let t = TestVm::new();
    let mut s = Script::new();
    for name in [
        "PHP_URL_SCHEME",
        "PHP_URL_HOST",
        "PHP_URL_PORT",
        "PHP_URL_USER",
        "PHP_URL_PASS",
        "PHP_URL_PATH",
        "PHP_URL_QUERY",
        "PHP_URL_FRAGMENT",
    ] {
        let Some(Value::Int(v)) = t.vm.constant(name) else {
            panic!("{name} is not registered")
        };
        s.echo(format!("{name}: {v} \n"));
    }
    s.echo("Done");
    s.check(phpt!("url/parse_url_basic_010.phpt"));
}

#[test]
fn parse_url_basic_011_phpt() {
    let mut s = Script::new();
    s.echo("*** Testing parse_url() :can not recognize port without scheme ***\n");
    for url in [
        "127.0.0.1:9999?",
        "127.0.0.1:9999#",
        "internal:#feeding",
        "magnet:?xt=urn:sha1:YNCKHTQCWBTRNJIV4WNAE52SJUQCZO5C",
    ] {
        s.echo(format!("parse {url}\n"));
        let v = s.parse_url(url.as_bytes(), None);
        s.var_dump(&v);
    }
    s.check(phpt!("url/parse_url_basic_011.phpt"));
}

#[test]
fn parse_url_error_002_phpt() {
    let url = b"http://secret:hideout@www.php.net:80/index.php?test=1&test2=char&test3=mixesCI#some_page_ref123";
    let mut s = Script::new();
    s.echo(
        "*** Testing parse_url() : error conditions: url component specifier out of range ***\n",
    );
    s.echo("--> Below range:");
    let v = s.parse_url(url, Some(-1));
    s.var_dump(&v);
    s.echo("\n\n--> Above range:\n");
    let r = s.try_call("parse_url", vec![Value::bytes(url), Value::Int(99)]);
    let (class, message) = thrown(r);
    s.echo(format!("{class}: {message}\n"));
    s.echo("Done");
    s.check(phpt!("url/parse_url_error_002.phpt"));
}

/// Tests whose script is a series of `var_dump(parse_url('literal'))` calls.
fn replay_var_dumps(phpt: &str) {
    let mut s = Script::new();
    for url in parse_url_literals(file_section(phpt)) {
        let v = s.parse_url(&url, None);
        s.var_dump(&v);
    }
    s.check(phpt);
}

#[test]
fn parse_url_var_dump_bug_phpts() {
    replay_var_dumps(phpt!("url/parse_url_relative_scheme.phpt"));
    replay_var_dumps(phpt!("url/bug54180.phpt"));
    replay_var_dumps(phpt!("url/bug55399.phpt"));
    replay_var_dumps(phpt!("url/bug63162.phpt"));
    replay_var_dumps(phpt!("url/bug69976.phpt"));
    replay_var_dumps(phpt!("url/bug73192.phpt"));
    replay_var_dumps(phpt!("url/bug74780.phpt"));
}

#[test]
fn bug68917_phpt() {
    let mut s = Script::new();
    for url in ["//example.org:81/hi?a=b#c=d", "//example.org/hi?a=b#c=d"] {
        let Value::Array(a) = s.parse_url(url.as_bytes(), None) else {
            panic!("array expected")
        };
        // print_r() of a flat array.
        s.echo("Array\n(\n");
        for (k, v) in a.iter() {
            let ArrayKey::Str(k) = k else {
                panic!("string keys")
            };
            s.echo(b"    [");
            s.echo(&k);
            s.echo(b"] => ");
            match v {
                Value::String(v) => s.echo(v),
                Value::Int(i) => s.echo(i.to_string()),
                other => panic!("unexpected {other:?}"),
            }
            s.echo(b"\n");
        }
        s.echo(")\n");
    }
    s.check(phpt!("url/bug68917.phpt"));
}

#[test]
fn url_t_phpt() {
    let phpt = phpt!("strings/url_t.phpt");
    let samples = quoted_lines(file_section(phpt));
    assert_eq!(samples.len(), 69);
    let mut s = Script::new();
    for url in &samples {
        s.echo(b"\n--> ");
        s.echo(url);
        s.echo(b": ");
        let v = s.parse_url(url, None);
        s.var_dump(&v);
    }
    let url = b"http://secret:hideout@www.php.net:80/index.php?test=1&test2=char&test3=mixesCI#some_page_ref123";
    for component in [
        PHP_URL_SCHEME,
        PHP_URL_HOST,
        PHP_URL_PORT,
        PHP_URL_USER,
        PHP_URL_PASS,
        PHP_URL_PATH,
        PHP_URL_QUERY,
        PHP_URL_FRAGMENT,
    ] {
        let v = s.parse_url(url, Some(component));
        s.var_dump(&v);
    }
    s.check(phpt);
}

// ---------------------------------------------------------------------------
// parse_url() edge cases traced through php_url_parse_ex2()
// ---------------------------------------------------------------------------

#[test]
fn parse_url_hosts_and_ports() {
    assert_eq!(
        dump(&parse("http://[::1]:8080/path", None)),
        "array(4) {\n  [\"scheme\"]=>\n  string(4) \"http\"\n  [\"host\"]=>\n  string(5) \"[::1]\"\n  \
         [\"port\"]=>\n  int(8080)\n  [\"path\"]=>\n  string(5) \"/path\"\n}\n"
    );
    assert_eq!(
        dump(&parse("http://[::1]/", Some(PHP_URL_HOST))),
        "string(5) \"[::1]\"\n"
    );
    assert_eq!(dump(&parse("http://[::1]", Some(PHP_URL_PORT))), "NULL\n");
    // Ports go through strtol(): a sign or trailing garbage are accepted.
    assert_eq!(
        dump(&parse("http://host:+80/", Some(PHP_URL_PORT))),
        "int(80)\n"
    );
    assert_eq!(
        dump(&parse("http://host:8a/", Some(PHP_URL_PORT))),
        "int(8)\n"
    );
    assert_eq!(dump(&parse("http://host:-1/", None)), "bool(false)\n");
    assert_eq!(dump(&parse("http://host:65536/", None)), "bool(false)\n");
    assert_eq!(
        dump(&parse("http://host:65535/", Some(PHP_URL_PORT))),
        "int(65535)\n"
    );
    assert_eq!(
        dump(&parse("http://example.com:0/", Some(PHP_URL_PORT))),
        "int(0)\n"
    );
    assert_eq!(dump(&parse("http://host:/", Some(PHP_URL_PORT))), "NULL\n");
    // A bare host:port; port 0 found first is re-parsed from the host part.
    assert_eq!(dump(&parse("a.com:0", Some(PHP_URL_PORT))), "int(0)\n");
    assert_eq!(
        dump(&parse("a.com:123456", None)),
        "array(2) {\n  [\"scheme\"]=>\n  string(5) \"a.com\"\n  [\"path\"]=>\n  string(6) \"123456\"\n}\n"
    );
    // ":80" finds a port but no host.
    assert_eq!(dump(&parse(":80", None)), "bool(false)\n");
}

#[test]
fn parse_url_misc_components() {
    // Control characters become '_' in every component.
    assert_eq!(
        dump(&parse("http://ho\u{1}st/pa\u{7f}th", Some(PHP_URL_HOST))),
        "string(5) \"ho_st\"\n"
    );
    assert_eq!(
        dump(&parse("http://ho\u{1}st/pa\u{7f}th", Some(PHP_URL_PATH))),
        "string(6) \"/pa_th\"\n"
    );
    // An invalid scheme character makes the whole string a path.
    assert_eq!(
        dump(&parse("sch\u{0}eme:", None)),
        "array(1) {\n  [\"path\"]=>\n  string(8) \"sch_eme:\"\n}\n"
    );
    // Windows drive letters in file URLs.
    assert_eq!(
        dump(&parse("file:///c:/dir/file.txt", Some(PHP_URL_PATH))),
        "string(15) \"c:/dir/file.txt\"\n"
    );
    assert_eq!(
        dump(&parse("FILE:///etc/passwd", Some(PHP_URL_PATH))),
        "string(11) \"/etc/passwd\"\n"
    );
    assert_eq!(
        dump(&parse("", None)),
        "array(1) {\n  [\"path\"]=>\n  string(0) \"\"\n}\n"
    );
    assert_eq!(
        dump(&parse("?#", None)),
        "array(2) {\n  [\"query\"]=>\n  string(0) \"\"\n  [\"fragment\"]=>\n  string(0) \"\"\n}\n"
    );
    // Bytes above 0x7f are kept verbatim.
    assert_eq!(
        dump(&parse("http://h\u{e9}/", Some(PHP_URL_HOST))),
        "string(3) \"h\u{e9}\"\n"
    );
    // Identifiers below -1 still select the whole array.
    assert!(matches!(parse("http://h/", Some(-5)), Value::Array(_)));
    // Absent components give NULL.
    assert_eq!(dump(&parse("http://h/", Some(PHP_URL_USER))), "NULL\n");

    let mut t = TestVm::new();
    let r = t.call("parse_url", vec![Value::str("x"), Value::Int(8)]);
    assert_eq!(
        thrown(r),
        (
            "ValueError".to_string(),
            "parse_url(): Argument #2 ($component) must be a valid URL component identifier, 8 given".to_string()
        )
    );
}

#[test]
fn php_url_parse_struct() {
    let url = php_url_parse(b"https://u:p@h:8443/a?b#c").expect("parses");
    assert_eq!(
        url,
        PhpUrl {
            scheme: Some(b"https".to_vec()),
            user: Some(b"u".to_vec()),
            pass: Some(b"p".to_vec()),
            host: Some(b"h".to_vec()),
            port: 8443,
            has_port: true,
            path: Some(b"/a".to_vec()),
            query: Some(b"b".to_vec()),
            fragment: Some(b"c".to_vec()),
        }
    );
    assert_eq!(php_url_parse(b"http:///x"), None);
}

// ---------------------------------------------------------------------------
// urlencode() and friends
// ---------------------------------------------------------------------------

#[test]
fn bug53248_phpt() {
    let mut s = Script::new();
    for (f, arg) in [
        ("rawurlencode", "A1_-.~"),
        ("rawurldecode", "%41%31%5F%2D%2E%7E"),
    ] {
        let v = s.try_call(f, vec![Value::str(arg)]).expect("string");
        s.var_dump(&v);
    }
    s.check(phpt!("url/bug53248.phpt"));
}

#[test]
fn bug14580_urlencode_part() {
    // echo strlen($key), ': ', urlencode($key) with $key = "foo\0bar".
    assert_eq!(call_str("urlencode", b"foo\0bar"), b"foo%00bar");
}

#[test]
fn urlencode_all_bytes() {
    let all: Vec<u8> = (0..=255).collect();
    let expect = |raw: bool| -> Vec<u8> {
        let mut out = Vec::new();
        for &c in &all {
            if c.is_ascii_alphanumeric() || b"-_.".contains(&c) || (raw && c == b'~') {
                out.push(c);
            } else if c == b' ' && !raw {
                out.push(b'+');
            } else {
                out.extend_from_slice(format!("%{c:02X}").as_bytes());
            }
        }
        out
    };
    assert_eq!(call_str("urlencode", &all), expect(false));
    assert_eq!(call_str("rawurlencode", &all), expect(true));
    // Decoding reverses both encodings.
    assert_eq!(call_str("urldecode", &expect(false)), all);
    assert_eq!(call_str("rawurldecode", &expect(true)), all);
}

#[test]
fn urlencode_examples() {
    assert_eq!(
        call_str("urlencode", b"a b&c=d/e?f~g"),
        b"a+b%26c%3Dd%2Fe%3Ff%7Eg"
    );
    assert_eq!(
        call_str("rawurlencode", b"a b&c=d/e?f~g"),
        b"a%20b%26c%3Dd%2Fe%3Ff~g"
    );
    assert_eq!(call_str("urlencode", b""), b"");
    assert_eq!(
        call_str("urlencode", "żółw".as_bytes()),
        b"%C5%BC%C3%B3%C5%82w"
    );
}

#[test]
fn urldecode_examples() {
    assert_eq!(call_str("urldecode", b"a+b%20c%2"), b"a b c%2");
    assert_eq!(call_str("rawurldecode", b"a+b%20c"), b"a+b c");
    assert_eq!(call_str("urldecode", b"%zz%4"), b"%zz%4");
    assert_eq!(call_str("urldecode", b"%41%4a%4A%"), b"AJJ%");
    assert_eq!(call_str("urldecode", b"%%41"), b"%A");
    assert_eq!(call_str("urldecode", b"%FF%00x"), b"\xff\0x");
    assert_eq!(call_str("rawurldecode", b"%4"), b"%4");
    assert_eq!(call_str("rawurldecode", b"%"), b"%");
}

#[test]
fn url_helpers() {
    assert_eq!(php_url_encode(b"a b~"), b"a+b%7E");
    assert_eq!(php_raw_url_encode(b"a b~"), b"a%20b~");
    let mut buf = b"x=".to_vec();
    php_url_encode_to(&mut buf, b"1 2", false);
    php_url_encode_to(&mut buf, b"&3 4", true);
    assert_eq!(buf, b"x=1+2%263%204");
    assert_eq!(php_url_decode(b"a+%41"), b"a A");
    assert_eq!(php_raw_url_decode(b"a+%41"), b"a+A");
    let mut buf = b"%41%42+c".to_vec();
    assert_eq!(php_url_decode_in_place(&mut buf), 4);
    assert_eq!(buf, b"AB c");
    let mut buf = b"%41%42+c".to_vec();
    assert_eq!(php_raw_url_decode_in_place(&mut buf), 4);
    assert_eq!(buf, b"AB+c");
    assert_eq!((PHP_QUERY_RFC1738, PHP_QUERY_RFC3986), (1, 2));
}
