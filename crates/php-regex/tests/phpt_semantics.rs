//! Behaviour checks derived from php-src's ext/pcre/tests/*.phpt, using a
//! minimal re-implementation of PHP's preg_match_all()/preg_split() loops
//! (see php_pcre_match_impl() / php_pcre_split_impl()).

use std::fs;
use std::path::PathBuf;

use php_regex::{MatchError, MatchOptions, PregError, Regex, compile_php_pattern};

fn opts(backtrack: u32, recursion: u32) -> MatchOptions {
    MatchOptions {
        match_limit: backtrack,
        depth_limit: recursion,
        ..MatchOptions::default()
    }
}

/// PHP's global matching loop. Returns all match ranges or the error.
fn match_all(
    re: &Regex,
    subject: &[u8],
    o: MatchOptions,
) -> Result<Vec<(usize, usize)>, MatchError> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut first = true;
    loop {
        let mut mo = o;
        mo.no_utf_check = !first;
        first = false;
        let Some(c) = re.match_at(subject, start, mo)? else {
            break;
        };
        let (s, e) = c.get(0).unwrap();
        out.push((s, e));
        start = e;
        if e == s {
            // Empty match: retry anchored with NOTEMPTY_ATSTART, else advance.
            loop {
                let mut mo2 = o;
                mo2.no_utf_check = true;
                mo2.not_empty_atstart = true;
                mo2.anchored = true;
                match re.match_at(subject, start, mo2)? {
                    Some(c) => {
                        let (s2, e2) = c.get(0).unwrap();
                        out.push((s2, e2));
                        start = e2;
                        if e2 != s2 {
                            break;
                        }
                    }
                    None => {
                        if start >= subject.len() {
                            return Ok(out);
                        }
                        start += if re.is_utf() {
                            let mut l = 1;
                            while start + l < subject.len() && subject[start + l] & 0xc0 == 0x80 {
                                l += 1;
                            }
                            l
                        } else {
                            1
                        };
                        break;
                    }
                }
            }
        }
        if start > subject.len() {
            break;
        }
    }
    Ok(out)
}

fn php(p: &str) -> Regex {
    compile_php_pattern(p.as_bytes()).unwrap().regex
}

#[test]
fn backtrack_limit_phpt() {
    // backtrack_limit.phpt / errors04.phpt: pcre.backtrack_limit=2
    let o = opts(2, 100_000);
    let e = match_all(&php(r"/.*\p{N}/"), b"0123456789", o).unwrap_err();
    assert_eq!(e.preg_error(), PregError::BacktrackLimit);
    assert_eq!(
        match_all(&php(r"/\p{Nd}/"), b"0123456789", o)
            .unwrap()
            .len(),
        10
    );
}

#[test]
fn recursion_limit_phpt() {
    // recursion_limit.phpt: pcre.recursion_limit=2
    let o = opts(1_000_000, 2);
    let e = match_all(&php(r"/\p{Ll}(\p{L}((\p{Ll}\p{Ll})))/"), b"aeiou", o).unwrap_err();
    assert_eq!(e.preg_error(), PregError::RecursionLimit);
    assert_eq!(
        match_all(&php(r"/\p{Ll}\p{L}\p{Ll}\p{Ll}/"), b"aeiou", o)
            .unwrap()
            .len(),
        1
    );
    // errors01.phpt / split2.phpt: pcre.recursion_limit=1
    let e = match_all(&php(r"/(\d*)/"), b"ab2c3u", opts(1_000_000, 1)).unwrap_err();
    assert_eq!(e.preg_error().message(), "Recursion limit exhausted");
}

#[test]
fn bug52732_backtrack_limit_1() {
    let re = php(r"/(?:\D+|<\d+>)*[!?]/");
    let e = re
        .match_at(b"foobar foobar foobar", 0, opts(1, 100_000))
        .unwrap_err();
    assert_eq!(e, MatchError::MatchLimit);
}

#[test]
fn split2_phpt_matches() {
    // preg_split('/(\d*)/', 'ab2c3u') splits at these matches.
    let m = match_all(&php(r"/(\d*)/"), b"ab2c3u", MatchOptions::default()).unwrap();
    assert_eq!(
        m,
        vec![(0, 0), (1, 1), (2, 3), (3, 3), (4, 5), (5, 5), (6, 6)]
    );
}

#[test]
fn bug006_catastrophic_hits_limit() {
    // 006.phpt: pcre.backtrack_limit=100000 makes both replacements fail.
    let text = b"[CODE]&lt;td align=&quot;$stylevar[right]&quot;&gt;[/CODE]";
    let re = php(r"#\[(right)\](((?R)|[^[]+?|\[)*)\[/\1\]#siU");
    let e = match_all(&re, text, opts(100_000, 100_000)).unwrap_err();
    assert_eq!(e.preg_error(), PregError::BacktrackLimit);
}

fn single_quoted_after(text: &str, marker: &str) -> Vec<u8> {
    let start = text.find(marker).unwrap() + marker.len();
    let b = text.as_bytes();
    let mut i = start;
    let mut v = Vec::new();
    while b[i] != b'\'' {
        if b[i] == b'\\' && (b[i + 1] == b'\\' || b[i + 1] == b'\'') {
            v.push(b[i + 1]);
            i += 2;
        } else {
            v.push(b[i]);
            i += 1;
        }
    }
    v
}

#[test]
fn preg_match_error4_large_json_pattern() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../php-src-ref/ext/pcre/tests/preg_match_error4.phpt");
    let Ok(text) = fs::read_to_string(&path) else {
        eprintln!("skipping: {} not found", path.display());
        return;
    };
    let re = single_quoted_after(&text, "$re = '");
    let subject = single_quoted_after(&text, "$str = '");
    let re = compile_php_pattern(&re).unwrap().regex;
    let r = re.match_at(&subject, 0, MatchOptions::default());
    assert!(
        matches!(r, Ok(Some(_))),
        "expected one match with PHP's default limits, got {:?}",
        r.map(|c| c.is_some())
    );
}

#[test]
fn invalid_utf8_phpt() {
    // invalid_utf8.phpt / invalid_utf8_offset.phpt
    let re = php("/aaa/u");
    let e = re
        .match_at(b"aaa\xc3", 0, MatchOptions::default())
        .unwrap_err();
    assert_eq!(e.preg_error(), PregError::BadUtf8);
    let re = php("/./u");
    let e = re
        .match_at("\u{e9}x".as_bytes(), 1, MatchOptions::default())
        .unwrap_err();
    assert_eq!(e.preg_error(), PregError::BadUtf8Offset);
    assert_eq!(
        e.preg_error().message(),
        "The offset did not correspond to the beginning of a valid UTF-8 code point"
    );
}

#[test]
fn bug20528_split_on_word_boundaries() {
    // preg_split('/\b/', "&lt;&gt;") style: boundaries in a string.
    let m = match_all(&php(r"/\b/"), b"ab cd", MatchOptions::default()).unwrap();
    assert_eq!(m, vec![(0, 0), (2, 2), (3, 3), (5, 5)]);
}

#[test]
fn utf8_split_empty_pattern() {
    // bug52971 / preg_split('//u', ...) splits into characters.
    let s = "abc\u{e9}\u{65e5}".as_bytes();
    let m = match_all(&php("//u"), s, MatchOptions::default()).unwrap();
    assert_eq!(m.len(), 6);
    assert_eq!(m[4], (5, 5));
}

#[test]
fn marks_phpt() {
    // marks.phpt
    let re = php("/(*MARK:A)a|(*MARK:B)b/");
    let c = re
        .match_at(b"b", 0, MatchOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(c.mark(), Some(&b"B"[..]));
    let re = php("/a(*MARK:X)b|c/");
    let c = re
        .match_at(b"c", 0, MatchOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(c.mark(), None);
}
