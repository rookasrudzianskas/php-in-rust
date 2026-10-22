//! Unit tests: API behaviour and PCRE2 10.43/10.44-specific semantics
//! (the bulk of the behavioural coverage is in tests/pcre2_cases.rs).

use super::*;

type Groups = Vec<Option<(usize, usize)>>;

fn re(pat: &str, flags: CompileFlags) -> Regex {
    Regex::new(pat.as_bytes(), flags).unwrap_or_else(|e| panic!("{pat}: {e}"))
}

fn m(pat: &str, subj: &str) -> Option<Groups> {
    mf(pat, CompileFlags::empty(), subj)
}

fn mf(pat: &str, flags: CompileFlags, subj: &str) -> Option<Groups> {
    re(pat, flags)
        .match_at(subj.as_bytes(), 0, MatchOptions::default())
        .unwrap()
        .map(|c| c.iter().collect())
}

fn mu(pat: &str, subj: &str) -> Option<Groups> {
    mf(pat, CompileFlags::UTF | CompileFlags::UCP, subj)
}

fn err(pat: &str) -> (i32, usize, &'static str) {
    let e = Regex::new(pat.as_bytes(), CompileFlags::empty()).unwrap_err();
    (e.code, e.offset, e.message())
}

fn php_match(p: &str, s: &str) -> bool {
    compile_php_pattern(p.as_bytes())
        .unwrap()
        .regex
        .is_match(s.as_bytes())
}

#[test]
fn smoke() {
    assert_eq!(m("abc", "xxabcxx"), Some(vec![Some((2, 5))]));
    assert_eq!(m("a(b)c", "abc"), Some(vec![Some((0, 3)), Some((1, 2))]));
    assert_eq!(m("x", "abc"), None);
}

#[test]
fn unset_vs_empty_groups_and_count() {
    let r = re("(a)|(b)(c)?(d*)", CompileFlags::empty());
    let c = r
        .match_at(b"a", 0, MatchOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(c.count(), 2);
    assert_eq!(c.len(), 5);
    assert_eq!(c.get(1), Some((0, 1)));
    assert_eq!(c.get(2), None);
    let c = r
        .match_at(b"b", 0, MatchOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!(c.count(), 5);
    assert_eq!(c.get(1), None);
    assert_eq!(c.get(3), None);
    assert_eq!(c.get(4), Some((1, 1)));
}

#[test]
fn capture_names_order_and_dupnames() {
    let r = re("(?<zeta>a)(?<alpha>b)(c)(?<al>d)", CompileFlags::empty());
    assert_eq!(r.capture_count(), 4);
    assert_eq!(
        r.capture_names(),
        vec![
            (4, b"al".to_vec()),
            (2, b"alpha".to_vec()),
            (1, b"zeta".to_vec())
        ]
    );
    assert_eq!(r.group_number(b"alpha"), Some(2));
    let r = re("(?<n>a)|(?<n>b)|(?<m>c)", CompileFlags::DUPNAMES);
    assert_eq!(
        r.capture_names(),
        vec![(3, b"m".to_vec()), (1, b"n".to_vec()), (2, b"n".to_vec())]
    );
    // Back reference to a duplicated name uses the first set group.
    assert_eq!(
        mf("(?:(?<n>a)|(?<n>b))\\k<n>", CompileFlags::DUPNAMES, "bb"),
        Some(vec![Some((0, 2)), None, Some((0, 1))])
    );
    let e = Regex::new(b"(?<a>x)(?<a>y)", CompileFlags::empty()).unwrap_err();
    assert_eq!(
        e.message(),
        "two named subpatterns have the same name (PCRE2_DUPNAMES not set)"
    );
    // Branch reset groups with the same name and number are fine.
    let r = re("(?|(?<a>x)|(?<a>y))", CompileFlags::empty());
    assert_eq!(r.capture_names(), vec![(1, b"a".to_vec())]);
}

#[test]
fn compile_error_texts_and_offsets() {
    assert_eq!(err("(ab"), (114, 3, "missing closing parenthesis"));
    assert_eq!(err("ab)"), (122, 2, "unmatched closing parenthesis"));
    assert_eq!(
        err("[a"),
        (106, 2, "missing terminating ] for character class")
    );
    assert_eq!(
        err("+a"),
        (109, 0, "quantifier does not follow a repeatable item")
    );
    assert_eq!(
        err("a{3,2}"),
        (104, 5, "numbers out of order in {} quantifier")
    );
    assert_eq!(err("\\"), (101, 1, "\\ at end of pattern"));
    assert_eq!(err("\\i"), (103, 1, "unrecognized character follows \\"));
    assert_eq!(
        err("(?<=a+)b"),
        (125, 0, "length of lookbehind assertion is not limited")
    );
    assert_eq!(
        err("(?<=x{1,300})y"),
        (
            200,
            0,
            "branch too long in variable-length lookbehind assertion"
        )
    );
    assert_eq!(
        err("(a)(?(1)a|b|c)"),
        (
            127,
            3,
            "conditional subpattern contains more than two branches"
        )
    );
    assert_eq!(
        err("\\k<x>"),
        (115, 3, "reference to non-existent subpattern")
    );
    assert_eq!(
        err("(?<1a>x)"),
        (144, 3, "subpattern name must start with a non-digit")
    );
    assert_eq!(
        err("(?=\\K)"),
        (
            199,
            6,
            "\\K is not allowed in lookarounds (but see PCRE2_EXTRA_ALLOW_LOOKAROUND_BSK)"
        )
    );
    assert_eq!(err("[[:foo:]]"), (130, 3, "unknown POSIX class name"));
    assert_eq!(
        err("\\p{Foo}"),
        (147, 7, "unknown property after \\P or \\p")
    );
    assert_eq!(
        err("(*FOO)"),
        (160, 5, "(*VERB) not recognized or malformed")
    );
    assert_eq!(
        err("\\x{110000}"),
        (
            134,
            9,
            "character code point value in \\x{} or \\o{} is too large"
        )
    );
    assert_eq!(
        err("(?(?x)a)"),
        (128, 2, "assertion expected after (?( or (?(?C)")
    );
    let e = Regex::new(b"\\C", CompileFlags::UTF | CompileFlags::NEVER_BACKSLASH_C).unwrap_err();
    assert_eq!(
        e.php_message(),
        "using \\C is incompatible with the 'u' modifier"
    );
    assert_eq!(e.offset, 2);
    // Invalid UTF-8 in the pattern.
    let e = Regex::new(b"a\xff", CompileFlags::UTF).unwrap_err();
    assert_eq!((e.code, e.offset), (-23, 1));
    assert_eq!(e.message(), "UTF-8 error: illegal byte (0xfe or 0xff)");
    // bug74183.phpt: a group repeated too many times.
    let p = "([A-Z]|[a-z]|[0-9]| |\u{d1}|\u{f1}|!|&quot;|%|&amp;|'|\u{b4}|-|:|;|>|=|&lt;|@|_|,|\\{|\\}|`|~|\u{e1}|\u{e9}|\u{ed}|\u{f3}|\u{fa}|\u{c1}|\u{c9}|\u{cd}|\u{d3}|\u{da}|\u{fc}|\u{dc}){1,300}";
    let e = Regex::new(p.as_bytes(), CompileFlags::empty()).unwrap_err();
    assert_eq!(e.message(), "regular expression is too large");
}

#[test]
fn pcre2_10_43_features() {
    // Variable-length lookbehind (max 255 characters).
    assert_eq!(m("(?<=\\d{3,5})x", "12345x"), Some(vec![Some((5, 6))]));
    assert_eq!(m("(?<=ab|c{2,3})d", "cccd"), Some(vec![Some((3, 4))]));
    assert_eq!(m("(?<!\\d{2,4})x", "12x"), None);
    assert_eq!(
        m("(?<=(\\d{1,3}))x", "1234x"),
        Some(vec![Some((4, 5)), Some((1, 4))])
    );
    // {,m} quantifier.
    assert_eq!(m("x{,3}", "xxxx"), Some(vec![Some((0, 3))]));
    assert_eq!(m("^a{,2}$", "aaa"), None);
    // Spaces inside \x{ } and quantifier braces.
    assert_eq!(m("\\x{ 41 }", "A"), Some(vec![Some((0, 1))]));
    assert_eq!(m("a{ 2 , 3 }", "aaa"), Some(vec![Some((0, 3))]));
    // \w and \b include Mn and Pc in UCP mode.
    assert_eq!(mu("\\w+", "e\u{301}x"), Some(vec![Some((0, 4))]));
    assert_eq!(mu("a\\b", "a\u{300}"), None);
}

#[test]
fn caseless_restrict_phpt() {
    // preg_match_caseless_restrict.phpt
    let cases: &[(&str, &str, bool)] = &[
        ("/AskZ/iur", "AskZ", true),
        ("/AskZ/iur", "aSKz", true),
        ("/AskZ/iur", "A\u{17f}kZ", false),
        ("/AskZ/iur", "As\u{212a}Z", false),
        ("/AskZ/iu", "A\u{17f}kZ", true),
        ("/AskZ/iu", "As\u{212a}Z", true),
        ("/k/iu", "\u{212A}", true),
        ("/k/iur", "\u{212A}", false),
        ("/A\\x{17f}\\x{212a}Z/iu", "AskZ", true),
        ("/A\\x{17f}\\x{212a}Z/iur", "AskZ", false),
        ("/[AskZ]+/iur", "As\u{212a}Z", true),
        ("/[\\x{17f}\\x{212a}]+/iur", "AskZ", false),
        ("/[\\x{17f}\\x{212a}]+/iu", "AskZ", true),
        ("/[^s]+/iur", "A\u{17f}Z", true),
        ("/[^k]+/iu", "A\u{212a}Z", true),
        ("/[^\\x{17f}]+/iur", "AsSZ", true),
        ("/s(?r)s(?-r)s(?r:s)s/iu", "\u{17f}S\u{17f}S\u{17f}", true),
        (
            "/s(?r)s(?-r)s(?r:s)s/iu",
            "\u{17f}\u{17f}\u{17f}S\u{17f}",
            false,
        ),
        (
            "/s(?r)s(?-r)s(?r:s)s/iu",
            "\u{17f}S\u{17f}\u{17f}\u{17f}",
            false,
        ),
        ("/k(?^i)k/iur", "K\u{212a}", true),
        ("/k(?^i)k/iur", "\u{212a}\u{212a}", false),
    ];
    for (p, s, want) in cases {
        assert_eq!(php_match(p, s), *want, "{p} on {s:?}");
    }
}

#[test]
fn unicode_caseless() {
    assert!(mu("(?i)\u{3c3}+", "\u{3a3}\u{3c2}\u{3c3}").is_some());
    assert_eq!(
        mu("(?i)^\u{3c3}+$", "\u{3a3}\u{3c2}\u{3c3}"),
        Some(vec![Some((0, 6))])
    );
    assert_eq!(
        mu("(?i)stra\u{df}e", "STRA\u{1e9e}E"),
        Some(vec![Some((0, 8))])
    );
    assert_eq!(mu("(?i)\u{e9}cole", "\u{c9}COLE"), Some(vec![Some((0, 6))]));
    assert_eq!(
        mu("(?i)(.)\\1", "\u{c9}\u{e9}"),
        Some(vec![Some((0, 4)), Some((0, 2))])
    );
    assert_eq!(
        mu("(?i)[\u{e0}-\u{ff}]+", "\u{c0}\u{dd}\u{178}"),
        Some(vec![Some((0, 6))])
    );
    // No Unicode case folding without UTF/UCP.
    assert_eq!(mf("(?i)\u{e9}", CompileFlags::empty(), "\u{c9}"), None);
}

#[test]
fn unicode_properties_and_extended_graphemes() {
    assert_eq!(
        mu("\\p{Greek}+", "x\u{3b1}\u{3b2}y"),
        Some(vec![Some((1, 5))])
    );
    assert_eq!(
        mu("\\p{Han}+", "\u{65e5}\u{672c}"),
        Some(vec![Some((0, 6))])
    );
    assert_eq!(mu("^\\X$", "e\u{301}"), Some(vec![Some((0, 3))]));
    assert_eq!(
        mu("^\\X$", "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"),
        Some(vec![Some((0, 18))])
    );
    assert_eq!(
        mu("^\\X\\X$", "\u{1f1fa}\u{1f1f8}\u{1f1ec}\u{1f1e7}"),
        Some(vec![Some((0, 16))])
    );
    assert_eq!(mu("^\\X+?$", "a\r\nb"), Some(vec![Some((0, 4))]));
    assert_eq!(mu("\\d+", "\u{661}\u{662}3"), Some(vec![Some((0, 5))]));
    assert_eq!(
        mf("\\d+", CompileFlags::UTF, "\u{661}\u{662}3"),
        Some(vec![Some((4, 5))])
    );
    assert_eq!(
        mu("[[:alpha:]]+", "\u{e9}t\u{e9}"),
        Some(vec![Some((0, 5))])
    );
    assert_eq!(
        mu("\\p{Lu}\\p{Ll}+", "x\u{c9}t\u{e9}"),
        Some(vec![Some((1, 6))])
    );
    assert_eq!(mu("\\p{L&}+", "aB\u{1c5}1"), Some(vec![Some((0, 4))]));
    assert_eq!(mu("\\p{scx:Arabic}", "\u{640}"), Some(vec![Some((0, 2))]));
    assert_eq!(mu("\\p{sc:Arabic}", "\u{640}"), None);
    assert_eq!(mu("\\p{Bidi_Class:R}", "\u{5d0}"), Some(vec![Some((0, 2))]));
    assert_eq!(mu("\\p{Alphabetic}+", "ab1"), Some(vec![Some((0, 2))]));
}

#[test]
fn verbs_and_marks() {
    // (*SKIP:A) jumps to the position of (*MARK:A), skipping "ac".
    let r = re("(*MARK:A)a(*SKIP:A)b|ac", CompileFlags::empty());
    assert!(
        r.match_at(b"ac", 0, MatchOptions::default())
            .unwrap()
            .is_none()
    );
    let r = re("a(*MARK:A)b(*SKIP:A)c|abd", CompileFlags::empty());
    assert!(
        r.match_at(b"abd", 0, MatchOptions::default())
            .unwrap()
            .is_none()
    );
    let r = re("a(*MARK:M1)b(*COMMIT)c|abd", CompileFlags::empty());
    match r
        .match_at_with_mark(b"abd", 0, MatchOptions::default())
        .unwrap()
    {
        Err(mark) => assert_eq!(mark, Some(b"M1".to_vec())),
        Ok(_) => panic!("COMMIT should prevent the second alternative"),
    }
    assert_eq!(m("(*COMMIT)abc", "xyzabc"), Some(vec![Some((3, 6))]));
    assert_eq!(m("(*NO_START_OPT)(*COMMIT)abc", "xyzabc"), None);
    assert_eq!(m("a(*THEN)b|ac", "ac"), Some(vec![Some((0, 2))]));
    assert_eq!(m("x(*ACCEPT)y", "xz"), Some(vec![Some((0, 1))]));
    assert_eq!(
        m("(a(*ACCEPT)b)c", "ax"),
        Some(vec![Some((0, 1)), Some((0, 1))])
    );
}

#[test]
fn recursion_and_conditions() {
    assert_eq!(
        m("\\((?:[^()]|(?R))*\\)", "x((a)(b))y"),
        Some(vec![Some((1, 9))])
    );
    assert_eq!(
        m("^(a(?1)?b)$", "aaabbb"),
        Some(vec![Some((0, 6)), Some((0, 6))])
    );
    assert_eq!(
        m("(?(DEFINE)(?<d>\\d{2}))(?&d)-(?&d)", "12-34"),
        Some(vec![Some((0, 5)), None])
    );
    assert_eq!(
        m("^(?:(a)|b)(?(1)c|d)$", "bd"),
        Some(vec![Some((0, 2)), None])
    );
    assert_eq!(m("(?(?=\\d)\\d+|[a-z]+)", "abc"), Some(vec![Some((0, 3))]));
    assert_eq!(
        m("^((.)(?1)\\2|.)$", "abcba"),
        Some(vec![Some((0, 5)), Some((0, 5)), Some((0, 1))])
    );
}

#[test]
fn match_options_and_offsets() {
    let r = re("a*", CompileFlags::empty());
    let o = MatchOptions {
        not_empty_atstart: true,
        anchored: true,
        ..MatchOptions::default()
    };
    assert!(r.match_at(b"b", 0, o).unwrap().is_none());
    assert_eq!(
        r.match_at(b"aab", 0, o).unwrap().unwrap().get(0),
        Some((0, 2))
    );
    let r = re("\\Gab", CompileFlags::empty());
    assert_eq!(
        r.match_at(b"xxab", 2, MatchOptions::default())
            .unwrap()
            .unwrap()
            .get(0),
        Some((2, 4))
    );
    assert!(
        r.match_at(b"xxab", 1, MatchOptions::default())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        re("x", CompileFlags::empty()).match_at(b"ab", 3, MatchOptions::default()),
        Err(MatchError::BadOffset)
    );
    let r = re("(?<=a)b", CompileFlags::empty());
    assert_eq!(
        r.match_at(b"ab", 1, MatchOptions::default())
            .unwrap()
            .unwrap()
            .get(0),
        Some((1, 2))
    );
    let r = re("a\\Kb", CompileFlags::empty());
    let c = r
        .match_at(b"ab", 0, MatchOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!((c.start(), c.end()), (1, 2));
}

#[test]
fn utf8_subject_errors() {
    let r = re(".", CompileFlags::UTF);
    assert_eq!(
        r.match_at(b"ab\xc3", 0, MatchOptions::default()),
        Err(MatchError::BadUtf8 {
            code: -3,
            offset: 2
        })
    );
    assert_eq!(
        r.match_at("\u{e9}".as_bytes(), 1, MatchOptions::default()),
        Err(MatchError::BadUtf8Offset)
    );
    assert_eq!(
        MatchError::BadUtf8Offset.preg_error(),
        PregError::BadUtf8Offset
    );
    // Offsets are byte offsets.
    let c = re("\u{65e5}(.)", CompileFlags::UTF)
        .match_at("a\u{65e5}\u{672c}".as_bytes(), 0, MatchOptions::default())
        .unwrap()
        .unwrap();
    assert_eq!((c.get(0), c.get(1)), (Some((1, 7)), Some((4, 7))));
}

#[test]
fn limits() {
    let r = re("(a+)+b", CompileFlags::empty());
    let o = MatchOptions {
        match_limit: 1000,
        ..MatchOptions::default()
    };
    assert_eq!(
        r.match_at(b"aaaaaaaaaaaaaaaaaaaaaaaaacb", 0, o),
        Err(MatchError::MatchLimit)
    );
    assert_eq!(
        MatchError::MatchLimit.preg_error().message(),
        "Backtrack limit exhausted"
    );
    let r = re("(((a)))", CompileFlags::empty());
    let o = MatchOptions {
        depth_limit: 3,
        ..MatchOptions::default()
    };
    assert_eq!(r.match_at(b"a", 0, o), Err(MatchError::DepthLimit));
    // Limits set in the pattern only lower the caller's limits.
    let r = re("(*LIMIT_MATCH=10)(a+)+b", CompileFlags::empty());
    assert_eq!(
        r.match_at(b"aaaaaacb", 0, MatchOptions::default()),
        Err(MatchError::MatchLimit)
    );
    // Deep backtracking does not overflow the native stack.
    let long = "a".repeat(200_000);
    let r = re("(?:a|b)*c", CompileFlags::empty());
    let o = MatchOptions {
        match_limit: u32::MAX,
        depth_limit: u32::MAX,
        ..MatchOptions::default()
    };
    assert_eq!(r.match_at(long.as_bytes(), 0, o).unwrap(), None);
}

#[test]
fn newline_conventions() {
    assert_eq!(m("(*CRLF)(?m)^b", "a\r\nb"), Some(vec![Some((3, 4))]));
    assert_eq!(m("(*CR)a$", "a\r"), Some(vec![Some((0, 1))]));
    assert_eq!(m("(*ANYCRLF).+", "ab\rc"), Some(vec![Some((0, 2))]));
    // Non-UTF: the bytes C2 85 contain the NEL byte 0x85.
    assert_eq!(m("(*ANY)(?m)^c", "a\u{85}c"), Some(vec![Some((3, 4))]));
    assert_eq!(m("(*LF)(?m)^c", "a\u{85}c"), None);
    assert_eq!(
        mf("(*ANY)(?m)^c", CompileFlags::UTF, "a\u{85}c"),
        Some(vec![Some((3, 4))])
    );
    assert_eq!(m("a$", "a\n"), Some(vec![Some((0, 1))]));
    assert_eq!(mf("a$", CompileFlags::DOLLAR_ENDONLY, "a\n"), None);
    assert_eq!(m("(*BSR_ANYCRLF)\\R", "\u{b}"), None);
}

#[test]
fn php_front_end() {
    let p = compile_php_pattern(b"/(?<w>\\w+)/u").unwrap();
    assert!(p.modifiers.utf);
    assert!(p.regex.is_utf());
    assert_eq!(p.regex.capture_names(), vec![(1, b"w".to_vec())]);
    let e = compile_php_pattern(b"/abc").unwrap_err();
    assert_eq!(e.to_string(), "No ending delimiter '/' found");
    let e = compile_php_pattern(b"/[a/").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Compilation failed: missing terminating ] for character class at offset 2"
    );
    let e = compile_php_pattern(b"/\\C/u").unwrap_err();
    assert_eq!(
        e.to_string(),
        "Compilation failed: using \\C is incompatible with the 'u' modifier at offset 2"
    );
    assert!(compile_php_pattern(b"/a/SX").is_ok());
    assert_eq!(
        compile_php_pattern(b"/a/F").unwrap_err().message_bytes(),
        b"Unknown modifier 'F'".to_vec()
    );
}

#[test]
fn auto_possessification_preserves_results() {
    for (p, s) in [
        ("a+b", "aaab"),
        ("\\d+\\s", "123 "),
        ("[^\"]+\"", "abc\""),
        ("\\w+(?=x)", "abcx"),
        ("(?:a+)b", "aab"),
        ("a*a", "aaa"),
    ] {
        let a = mf(p, CompileFlags::empty(), s);
        let b = mf(p, CompileFlags::NO_AUTO_POSSESS, s);
        assert_eq!(a, b, "{p}");
    }
}
