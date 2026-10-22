//! Tests of html.c. Most are ports of php-src's .phpt tests: each test replays the PHP script
//! through the native functions and compares the output with the EXPECT/EXPECTF section of the
//! vendored phpt file (`tests/phpt/ext/standard/tests/strings/`).

mod decode;
mod encode;
mod table;
mod unit;

use php_runtime::{ArrayKey, Value};

use crate::testing::TestVm;

/// A string value.
pub fn s(b: impl AsRef<[u8]>) -> Value {
    Value::bytes(b.as_ref())
}

/// An int value.
pub fn i(n: i64) -> Value {
    Value::Int(n)
}

pub fn bin2hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// UTF-8 encoding of any code point (surrogates included), like the phpt helpers.
pub fn utf8(cp: u32) -> Vec<u8> {
    let mut v = Vec::new();
    super::push_utf8(&mut v, cp);
    v
}

/// The bytes of a string value.
pub fn bytes(v: &Value) -> Vec<u8> {
    match v.as_str() {
        Some(s) => s.to_vec(),
        None => panic!("expected a string, got {v:?}"),
    }
}

/// A VM recording the output of a replayed phpt script.
pub struct Php {
    pub t: TestVm,
}

impl Php {
    pub fn new() -> Php {
        Php { t: TestVm::new() }
    }

    /// `ini_set()`, which must succeed.
    pub fn ini(&mut self, name: &str, value: &str) {
        assert!(self.t.vm.ini_set(name, value).is_some(), "ini_set({name})");
    }

    pub fn call(&mut self, f: &str, args: Vec<Value>) -> Value {
        match self.t.call(f, args) {
            Ok(v) => v,
            Err(e) => panic!("{f}() failed: {e:?}"),
        }
    }

    /// Calls a function returning a string.
    pub fn str(&mut self, f: &str, args: Vec<Value>) -> Vec<u8> {
        bytes(&self.call(f, args))
    }

    pub fn echo(&mut self, b: impl AsRef<[u8]>) {
        self.t.vm.echo(b.as_ref()).unwrap();
    }

    pub fn var_dump(&mut self, v: &Value) {
        let mut out = Vec::new();
        var_dump_into(&mut out, v, 0);
        self.echo(out);
    }

    /// `var_dump()` of a string.
    pub fn dump(&mut self, b: impl AsRef<[u8]>) {
        self.var_dump(&s(b));
    }

    pub fn print_r(&mut self, v: &Value) {
        let mut out = Vec::new();
        print_r_into(&mut out, v, 0);
        self.echo(out);
    }

    /// Everything output so far (echoes and diagnostics).
    pub fn output(&self) -> Vec<u8> {
        self.t.out.take()
    }

    /// Compares the output with the expectation of a phpt file.
    pub fn check(&self, phpt_name: &str) {
        let out = self.output();
        check_phpt(phpt_name, &out);
    }
}

fn key_bytes(k: &ArrayKey) -> Vec<u8> {
    match k {
        ArrayKey::Int(n) => n.to_string().into_bytes(),
        ArrayKey::Str(s) => s.to_vec(),
    }
}

fn var_dump_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    out.extend_from_slice(pad.as_bytes());
    match v {
        Value::Null => out.extend_from_slice(b"NULL\n"),
        Value::Bool(b) => out.extend_from_slice(format!("bool({b})\n").as_bytes()),
        Value::Int(n) => out.extend_from_slice(format!("int({n})\n").as_bytes()),
        Value::String(st) => {
            out.extend_from_slice(format!("string({}) \"", st.len()).as_bytes());
            out.extend_from_slice(st);
            out.extend_from_slice(b"\"\n");
        }
        Value::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            for (k, v) in a.iter() {
                out.extend_from_slice(pad.as_bytes());
                match &k {
                    ArrayKey::Int(n) => out.extend_from_slice(format!("  [{n}]=>\n").as_bytes()),
                    ArrayKey::Str(st) => {
                        out.extend_from_slice(b"  [\"");
                        out.extend_from_slice(st);
                        out.extend_from_slice(b"\"]=>\n");
                    }
                }
                var_dump_into(out, v, indent + 2);
            }
            out.extend_from_slice(pad.as_bytes());
            out.extend_from_slice(b"}\n");
        }
        other => panic!("var_dump of {other:?} is not supported by the test helper"),
    }
}

fn print_r_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
    match v {
        Value::Array(a) => {
            out.extend_from_slice(b"Array\n");
            out.extend_from_slice(" ".repeat(indent).as_bytes());
            out.extend_from_slice(b"(\n");
            for (k, v) in a.iter() {
                out.extend_from_slice(" ".repeat(indent + 4).as_bytes());
                out.push(b'[');
                out.extend_from_slice(&key_bytes(&k));
                out.extend_from_slice(b"] => ");
                print_r_into(out, v, indent + 8);
                out.push(b'\n');
            }
            out.extend_from_slice(" ".repeat(indent).as_bytes());
            out.extend_from_slice(b")\n");
        }
        Value::String(st) => out.extend_from_slice(st),
        Value::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
        other => panic!("print_r of {other:?} is not supported by the test helper"),
    }
}

/// `asort()` of an array of strings (stable, by byte comparison as the values are not
/// numeric).
pub fn asort(v: &Value) -> Value {
    let Value::Array(a) = v else {
        panic!("expected an array, got {v:?}");
    };
    let mut entries: Vec<(ArrayKey, Value)> = a.iter().map(|(k, v)| (k, v.clone())).collect();
    entries.sort_by_key(|e| bytes(&e.1));
    let mut sorted = php_runtime::PhpArray::new();
    for (k, v) in entries {
        sorted.insert(k, v);
    }
    Value::array(sorted)
}

/// Number of elements of an array value.
pub fn count(v: &Value) -> usize {
    match v {
        Value::Array(a) => a.len(),
        other => panic!("expected an array, got {other:?}"),
    }
}

/// `(key, value)` pairs of an array of strings.
pub fn entries(v: &Value) -> Vec<(Vec<u8>, Vec<u8>)> {
    match v {
        Value::Array(a) => a.iter().map(|(k, v)| (key_bytes(&k), bytes(v))).collect(),
        other => panic!("expected an array, got {other:?}"),
    }
}

// ----- phpt files --------------------------------------------------------------------------

fn phpt_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/phpt/ext/standard/tests/strings")
        .join(name)
}

/// The sections of a phpt file (name -> content).
pub fn phpt_sections(name: &str) -> Vec<(String, Vec<u8>)> {
    let path = phpt_path(name);
    let data =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut sections: Vec<(String, Vec<u8>)> = Vec::new();
    for line in data.split_inclusive(|&b| b == b'\n') {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
        if trimmed.len() > 4 && trimmed.starts_with(b"--") && trimmed.ends_with(b"--") {
            let title = &trimmed[2..trimmed.len() - 2];
            if title.iter().all(|b| b.is_ascii_uppercase() || *b == b'_') {
                sections.push((String::from_utf8(title.to_vec()).unwrap(), Vec::new()));
                continue;
            }
        }
        if let Some(last) = sections.last_mut() {
            last.1.extend_from_slice(line);
        }
    }
    sections
}

/// The FILE section (the PHP script) of a phpt file.
pub fn phpt_file(name: &str) -> Vec<u8> {
    phpt_sections(name)
        .into_iter()
        .find(|(n, _)| n == "FILE")
        .map(|(_, v)| v)
        .expect("FILE section")
}

/// Compares `out` with the EXPECT or EXPECTF section of a phpt file, as run-tests.php does
/// (both trimmed, line endings normalized).
pub fn check_phpt(name: &str, out: &[u8]) {
    let sections = phpt_sections(name);
    let (kind, expected) = sections
        .iter()
        .find(|(n, _)| n == "EXPECT" || n == "EXPECTF")
        .unwrap_or_else(|| panic!("{name}: no EXPECT section"));
    let norm = |b: &[u8]| {
        let s = String::from_utf8_lossy(b).replace("\r\n", "\n");
        s.trim().as_bytes().to_vec()
    };
    let (out_n, exp_n) = (norm(out), norm(expected));
    let ok = if kind == "EXPECTF" {
        expectf_matches(&exp_n, &out_n)
    } else {
        out_n == exp_n
    };
    if !ok {
        if let Some(dir) = std::env::var_os("HTML_PHPT_OUT") {
            // debugging aid: keep the output for diffing
            let _ = std::fs::write(
                std::path::Path::new(&dir).join(format!("{name}.out")),
                &out_n,
            );
            let _ = std::fs::write(
                std::path::Path::new(&dir).join(format!("{name}.exp")),
                &exp_n,
            );
        }
        let (o, e) = (
            String::from_utf8_lossy(&out_n),
            String::from_utf8_lossy(&exp_n),
        );
        let line = o.lines().zip(e.lines()).position(|(a, b)| a != b);
        panic!(
            "{name}: output differs from {kind} (first differing line: {line:?})\n--- output ---\n{o}\n--- expected ---\n{e}"
        );
    }
}

#[derive(Debug)]
enum Pat {
    Lit(Vec<u8>),
    /// Run of bytes satisfying the class, at least `min` long.
    Run(fn(u8) -> bool, usize),
    /// `%c`: one character.
    One,
    /// `%i`: optional sign and digits.
    Signed,
}

fn expectf_tokens(p: &[u8]) -> Vec<Pat> {
    let mut toks = Vec::new();
    let mut lit = Vec::new();
    let mut i = 0;
    while i < p.len() {
        if p[i] == b'%' && i + 1 < p.len() {
            let tok = match p[i + 1] {
                b's' => Some(Pat::Run(|b| b != b'\n' && b != b'\r', 1)),
                b'S' => Some(Pat::Run(|b| b != b'\n' && b != b'\r', 0)),
                b'a' => Some(Pat::Run(|_| true, 1)),
                b'A' => Some(Pat::Run(|_| true, 0)),
                b'w' => Some(Pat::Run(|b| b.is_ascii_whitespace(), 0)),
                b'd' => Some(Pat::Run(|b| b.is_ascii_digit(), 1)),
                b'x' => Some(Pat::Run(|b| b.is_ascii_hexdigit(), 1)),
                b'i' => Some(Pat::Signed),
                b'c' => Some(Pat::One),
                b'e' => {
                    lit.push(b'/');
                    i += 2;
                    continue;
                }
                b'0' => {
                    lit.push(0);
                    i += 2;
                    continue;
                }
                b'r' => panic!("%r is not supported by the test helper"),
                _ => None,
            };
            if let Some(tok) = tok {
                if !lit.is_empty() {
                    toks.push(Pat::Lit(std::mem::take(&mut lit)));
                }
                toks.push(tok);
                i += 2;
                continue;
            }
        }
        lit.push(p[i]);
        i += 1;
    }
    if !lit.is_empty() {
        toks.push(Pat::Lit(lit));
    }
    toks
}

fn match_tokens(toks: &[Pat], s: &[u8]) -> bool {
    let Some((first, rest)) = toks.split_first() else {
        return s.is_empty();
    };
    match first {
        Pat::Lit(l) => s.starts_with(l) && match_tokens(rest, &s[l.len()..]),
        Pat::Run(class, min) => {
            let max = s.iter().take_while(|&&b| class(b)).count();
            (*min..=max).rev().any(|n| match_tokens(rest, &s[n..]))
        }
        Pat::One => matches!(s.first(), Some(b) if *b != b'\n') && match_tokens(rest, &s[1..]),
        Pat::Signed => {
            let sign = usize::from(matches!(s.first(), Some(b'+' | b'-')));
            let max = s[sign..].iter().take_while(|b| b.is_ascii_digit()).count();
            (1..=max).rev().any(|n| match_tokens(rest, &s[sign + n..]))
        }
    }
}

pub fn expectf_matches(pattern: &[u8], s: &[u8]) -> bool {
    match_tokens(&expectf_tokens(pattern), s)
}

// ----- PHP literals in phpt scripts ----------------------------------------------------------

/// Decodes the body of a double-quoted PHP string literal (the escapes used by the tests).
pub fn php_dq(lit: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lit.len() {
        if lit[i] != b'\\' || i + 1 == lit.len() {
            out.push(lit[i]);
            i += 1;
            continue;
        }
        let c = lit[i + 1];
        i += 2;
        match c {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'v' => out.push(0x0B),
            b'f' => out.push(0x0C),
            b'e' => out.push(0x1B),
            b'\\' | b'$' | b'"' => out.push(c),
            b'x' if i < lit.len() && lit[i].is_ascii_hexdigit() => {
                let n = lit[i..]
                    .iter()
                    .take(2)
                    .take_while(|b| b.is_ascii_hexdigit())
                    .count();
                out.push(
                    u8::from_str_radix(std::str::from_utf8(&lit[i..i + n]).unwrap(), 16).unwrap(),
                );
                i += n;
            }
            b'0'..=b'7' => {
                let start = i - 1;
                let n = lit[start..]
                    .iter()
                    .take(3)
                    .take_while(|b| (b'0'..=b'7').contains(b))
                    .count();
                let v =
                    u32::from_str_radix(std::str::from_utf8(&lit[start..start + n]).unwrap(), 8)
                        .unwrap();
                out.push(v as u8);
                i = start + n;
            }
            _ => {
                out.push(b'\\');
                out.push(c);
            }
        }
    }
    out
}

/// The double-quoted string literals of `script` lines between the first line containing
/// `start` and the next line equal to `end` (e.g. the elements of a `$map = array(...)`).
pub fn dq_literals_between(script: &[u8], start: &str, end: &str) -> Vec<Vec<u8>> {
    let text = String::from_utf8_lossy(script).into_owned();
    let mut lines = text.lines().skip_while(|l| !l.contains(start));
    lines.next().expect("start line");
    let mut out = Vec::new();
    for line in lines {
        if line.trim() == end {
            break;
        }
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('"') {
            let close = rest.rfind('"').expect("closing quote");
            out.push(php_dq(&rest.as_bytes()[..close]));
        }
    }
    out
}

/// `0xU => array(0xB, "NAME"),` lines of a charset test script: (code point, byte, name).
pub fn charset_map_lines(script: &[u8]) -> Vec<(u32, u32, String)> {
    let text = String::from_utf8_lossy(script).into_owned();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("0x") else {
            continue;
        };
        let (u, rest) = rest.split_once(" => array(0x").expect("mapping line");
        let (b, rest) = rest.split_once(", \"").expect("mapping line");
        let name = rest.strip_suffix("\"),").expect("mapping line");
        out.push((
            u32::from_str_radix(u, 16).unwrap(),
            u32::from_str_radix(b, 16).unwrap(),
            name.to_string(),
        ));
    }
    out
}

#[test]
fn expectf_matcher() {
    assert!(expectf_matches(b"a %s b", b"a xyz b"));
    assert!(!expectf_matches(b"a %s b", b"a  b"));
    assert!(expectf_matches(b"in %s on line %d", b"in /t.php on line 3"));
    assert!(expectf_matches(b"x%0y", b"x\0y"));
    assert!(expectf_matches(b"%a!", b"a\nb!"));
    assert!(!expectf_matches(b"%d", b"x"));
    assert_eq!(php_dq(br#"a\x41\101\t\$\"\q"#), b"aAA\t$\"\\q");
}
