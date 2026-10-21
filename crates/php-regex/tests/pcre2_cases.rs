//! Replays `tests/data/pcre2_cases.txt`: thousands of pattern/subject cases
//! whose expected results (captures, marks, compile errors with offsets,
//! match-limit and depth-limit errors) were produced by the real PCRE2
//! library. Cases that depend on behaviour changed in PCRE2 10.43+ are not
//! in the file; those are covered by unit tests instead.

use php_regex::{CompileFlags, DEFAULT_HEAP_LIMIT, MatchOptions, Regex};

fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut v = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'\\' => v.push(b'\\'),
                b't' => v.push(b'\t'),
                b'n' => v.push(b'\n'),
                b'r' => v.push(b'\r'),
                b'x' => {
                    let h = std::str::from_utf8(&b[i + 2..i + 4]).unwrap();
                    v.push(u8::from_str_radix(h, 16).unwrap());
                    i += 4;
                    continue;
                }
                other => panic!("bad escape {}", other as char),
            }
            i += 2;
        } else {
            v.push(b[i]);
            i += 1;
        }
    }
    v
}

fn run_case(f: &[&str]) -> String {
    let pat = unescape(f[0]);
    let copt = u32::from_str_radix(f[1], 16).unwrap();
    let xopt = u32::from_str_radix(f[2], 16).unwrap();
    let subj = unescape(f[3]);
    let start: usize = f[4].parse().unwrap();
    let mopt = u32::from_str_radix(f[5], 16).unwrap();
    let re = match Regex::new(&pat, CompileFlags::from_pcre2(copt, xopt)) {
        Ok(r) => r,
        Err(e) => return format!("C {} {}", e.code, e.offset),
    };
    let mo = MatchOptions {
        anchored: mopt & 0x8000_0000 != 0,
        not_bol: mopt & 1 != 0,
        not_eol: mopt & 2 != 0,
        not_empty: mopt & 4 != 0,
        not_empty_atstart: mopt & 8 != 0,
        no_utf_check: false,
        match_limit: f[6].parse().unwrap(),
        depth_limit: f[7].parse().unwrap(),
        heap_limit: DEFAULT_HEAP_LIMIT,
    };
    match re.match_at(&subj, start, mo) {
        Ok(Some(c)) => {
            let mut s = format!("M {}", c.count());
            for i in 0..c.len() {
                match c.get(i) {
                    None => s.push_str(" -"),
                    Some((a, b)) => s.push_str(&format!(" {},{}", a, b)),
                }
            }
            s.push_str(" |");
            if let Some(m) = c.mark() {
                for b in m {
                    s.push_str(&format!("{:02x}", b));
                }
            }
            s
        }
        Ok(None) => "N".to_string(),
        Err(e) => format!("E {}", e.pcre2_code()),
    }
}

#[test]
fn pcre2_reference_cases() {
    let data = include_str!("data/pcre2_cases.txt");
    let mut n = 0;
    let mut failures = Vec::new();
    for line in data.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 9, "bad line {line:?}");
        n += 1;
        let got = run_case(&f);
        if got != f[8] {
            failures.push(format!("{line}\n    got: {got}"));
        }
    }
    assert!(n > 5000);
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        n,
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
