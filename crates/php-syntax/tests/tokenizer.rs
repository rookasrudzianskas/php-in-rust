//! `token_get_all()` compatibility tests.

use php_syntax::{TokenKind, TokenizeFlags, tokenize};

/// Format like `ext/tokenizer/tests/token_get_all_heredoc_nowdoc.phpt`.
fn dump(src: &str, parse: bool) -> String {
    match tokenize(src.as_bytes(), TokenizeFlags { parse }) {
        Ok(tokens) => {
            let mut out = String::new();
            for t in tokens {
                if t.kind.is_char() {
                    continue;
                }
                out.push_str(&format!(
                    "Line {}: {} ('{}')\n",
                    t.line,
                    t.kind.token_name(),
                    String::from_utf8_lossy(t.text(src.as_bytes()))
                ));
            }
            out
        }
        Err(e) => format!("ParseError: {} on line {}\n", e.message, e.line),
    }
}

const HEREDOC_CASES: [&str; 18] = [
    "<?php <<<INNER_END\nINNER_END;",
    "<?php <<<INNER_END\n  INNER_END;",
    "<?php <<<'INNER_END'\nINNER_END;",
    "<?php <<<'INNER_END'\n  INNER_END;",
    "<?php <<<INNER_END\na",
    "<?php <<<INNER_END\nab",
    "<?php <<<INNER_END\na\nINNER_END;",
    "<?php\n \t<<<'DOC'\n \tXXX\n \tDOC;",
    "<?php <<<INNER_END\nab\nINNER_END;",
    "<?php <<<INNER_END\n  ab\n  INNER_END;",
    "<?php <<<INNER_END\nabc\n   INNER_END;",
    "<?php <<<INNER_END\n\n   INNER_END;",
    "<?php <<<'INNER_END'\nab\nINNER_END;",
    "<?php <<<'INNER_END'\n  ab\n  INNER_END;",
    "<?php <<<'INNER_END'\nabc\n   INNER_END;",
    "<?php <<<'INNER_END'\n\n   INNER_END;",
    "<?php <<<INNER_END\nab\nab\n\t",
    "<?php <<<'INNER_END'\nab\nab\n\t",
];

const OPEN: &str = "Line 1: T_OPEN_TAG ('<?php ')\n";

fn start(nowdoc: bool) -> String {
    if nowdoc {
        "Line 1: T_START_HEREDOC ('<<<'INNER_END'\n')\n".to_string()
    } else {
        "Line 1: T_START_HEREDOC ('<<<INNER_END\n')\n".to_string()
    }
}

#[test]
fn heredoc_nowdoc_without_token_parse() {
    let s = |n| start(n);
    let expected: Vec<String> = vec![
        format!("{OPEN}{}Line 2: T_END_HEREDOC ('INNER_END')\n", s(false)),
        format!("{OPEN}{}Line 2: T_END_HEREDOC ('  INNER_END')\n", s(false)),
        format!("{OPEN}{}Line 2: T_END_HEREDOC ('INNER_END')\n", s(true)),
        format!("{OPEN}{}Line 2: T_END_HEREDOC ('  INNER_END')\n", s(true)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('a')\n", s(false)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('ab')\n", s(false)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('a\n')\nLine 3: T_END_HEREDOC ('INNER_END')\n", s(false)),
        "Line 1: T_OPEN_TAG ('<?php\n')\nLine 2: T_WHITESPACE (' \t')\nLine 2: T_START_HEREDOC ('<<<'DOC'\n')\nLine 3: T_ENCAPSED_AND_WHITESPACE (' \tXXX\n')\nLine 4: T_END_HEREDOC (' \tDOC')\n".to_string(),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('ab\n')\nLine 3: T_END_HEREDOC ('INNER_END')\n", s(false)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('  ab\n')\nLine 3: T_END_HEREDOC ('  INNER_END')\n", s(false)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('abc\n')\nLine 3: T_END_HEREDOC ('   INNER_END')\n", s(false)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('\n')\nLine 3: T_END_HEREDOC ('   INNER_END')\n", s(false)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('ab\n')\nLine 3: T_END_HEREDOC ('INNER_END')\n", s(true)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('  ab\n')\nLine 3: T_END_HEREDOC ('  INNER_END')\n", s(true)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('abc\n')\nLine 3: T_END_HEREDOC ('   INNER_END')\n", s(true)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('\n')\nLine 3: T_END_HEREDOC ('   INNER_END')\n", s(true)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('ab\nab\n\t')\n", s(false)),
        format!("{OPEN}{}Line 2: T_ENCAPSED_AND_WHITESPACE ('ab\nab\n\t')\n", s(true)),
    ];
    for (i, (src, exp)) in HEREDOC_CASES.iter().zip(&expected).enumerate() {
        assert_eq!(&dump(src, false), exp, "test case {}", i + 1);
    }
}

#[test]
fn heredoc_nowdoc_with_token_parse() {
    let eof = "ParseError: syntax error, unexpected end of file, expecting variable or heredoc end or \"${\" or \"{$\" on line";
    let ok = |i: usize| dump(HEREDOC_CASES[i], false);
    let expected: Vec<String> = vec![
        ok(0),
        ok(1),
        ok(2),
        ok(3),
        format!("{eof} 2\n"),
        format!("{eof} 2\n"),
        ok(6),
        "ParseError: Invalid indentation - tabs and spaces cannot be mixed on line 3\n".to_string(),
        ok(8),
        ok(9),
        "ParseError: Invalid body indentation level (expecting an indentation level of at least 3) on line 2\n"
            .to_string(),
        ok(11),
        ok(12),
        ok(13),
        "ParseError: Invalid body indentation level (expecting an indentation level of at least 3) on line 2\n"
            .to_string(),
        ok(15),
        format!("{eof} 4\n"),
        format!("{eof} 4\n"),
    ];
    for (i, (src, exp)) in HEREDOC_CASES.iter().zip(&expected).enumerate() {
        assert_eq!(&dump(src, true), exp, "test case {}", i + 1);
    }
}

#[test]
fn token_ids_follow_bison_numbering() {
    assert_eq!(TokenKind::End.id(), 0);
    assert_eq!(TokenKind::Throw.id(), 258);
    assert_eq!(TokenKind::Include.id(), 260);
    assert_eq!(TokenKind::LNumber.id(), 312);
    assert_eq!(TokenKind::String.id(), 314);
    assert_eq!(TokenKind::Variable.id(), 318);
    assert_eq!(TokenKind::Semicolon.id(), b';' as u16);
    assert_eq!(
        TokenKind::PaamayimNekudotayim.token_name(),
        "T_DOUBLE_COLON"
    );
    assert_eq!(TokenKind::Pipe.name(), Some("T_PIPE"));
    for &k in TokenKind::ALL {
        assert_eq!(TokenKind::from_id(k.id()), Some(k));
    }
}

#[test]
fn basic_stream() {
    let out = dump("<?php echo $a . 'b'; ?>\nhtml<?= 1 ?>", false);
    assert_eq!(
        out,
        "Line 1: T_OPEN_TAG ('<?php ')\nLine 1: T_ECHO ('echo')\nLine 1: T_WHITESPACE (' ')\nLine 1: T_VARIABLE ('$a')\nLine 1: T_WHITESPACE (' ')\nLine 1: T_WHITESPACE (' ')\nLine 1: T_CONSTANT_ENCAPSED_STRING (''b'')\nLine 1: T_WHITESPACE (' ')\nLine 1: T_CLOSE_TAG ('?>\n')\nLine 2: T_INLINE_HTML ('html')\nLine 2: T_OPEN_TAG_WITH_ECHO ('<?=')\nLine 2: T_WHITESPACE (' ')\nLine 2: T_LNUMBER ('1')\nLine 2: T_WHITESPACE (' ')\nLine 2: T_CLOSE_TAG ('?>')\n"
    );
}

#[test]
fn casts_and_operators() {
    let src = "<?php ( int )$a; (integer)$b; (real)$c; $a?->b; $x |> f(...); $a ??= 1; $a **= 2; yield from $g; #[A]";
    let names: Vec<&str> = tokenize(src.as_bytes(), TokenizeFlags::default())
        .unwrap()
        .iter()
        .filter(|t| !t.kind.is_char() && t.kind != TokenKind::Whitespace)
        .map(|t| t.kind.token_name())
        .collect();
    assert_eq!(
        names,
        [
            "T_OPEN_TAG",
            "T_INT_CAST",
            "T_VARIABLE",
            "T_INT_CAST",
            "T_VARIABLE",
            "T_DOUBLE_CAST",
            "T_VARIABLE",
            "T_VARIABLE",
            "T_NULLSAFE_OBJECT_OPERATOR",
            "T_STRING",
            "T_VARIABLE",
            "T_PIPE",
            "T_STRING",
            "T_ELLIPSIS",
            "T_VARIABLE",
            "T_COALESCE_EQUAL",
            "T_LNUMBER",
            "T_VARIABLE",
            "T_POW_EQUAL",
            "T_LNUMBER",
            "T_YIELD_FROM",
            "T_VARIABLE",
            "T_ATTRIBUTE",
            "T_STRING",
        ]
    );
}

#[test]
fn token_parse_reports_keywords_used_as_identifiers() {
    let src = "<?php class A { const LIST = 1; function list() {} } A::LIST; $a->class;";
    let toks = tokenize(src.as_bytes(), TokenizeFlags { parse: true }).unwrap();
    let texts: Vec<(&str, String)> = toks
        .iter()
        .filter(|t| !t.kind.is_char() && t.kind != TokenKind::Whitespace)
        .map(|t| {
            (
                t.kind.token_name(),
                String::from_utf8_lossy(t.text(src.as_bytes())).into_owned(),
            )
        })
        .collect();
    let lists: Vec<_> = texts
        .iter()
        .filter(|(_, s)| s == "LIST" || s == "list")
        .collect();
    assert!(lists.iter().all(|(n, _)| *n == "T_STRING"), "{lists:?}");
    // Without TOKEN_PARSE the keywords keep their keyword token.
    let raw = tokenize(src.as_bytes(), TokenizeFlags::default()).unwrap();
    assert!(raw.iter().any(|t| t.kind == TokenKind::List));
    let err = tokenize(b"<?php invalid code;", TokenizeFlags { parse: true }).unwrap_err();
    assert_eq!(err.message, "syntax error, unexpected identifier \"code\"");
}

#[test]
fn halt_compiler_rest_is_inline_html() {
    let src: &[u8] = b"<?php echo 1; __halt_compiler(); \x00binary\xff data";
    let toks = tokenize(src, TokenizeFlags::default()).unwrap();
    let last = toks.last().unwrap();
    assert_eq!(last.kind, TokenKind::InlineHtml);
    assert_eq!(last.text(src), b" \x00binary\xff data");
    let toks = tokenize(src, TokenizeFlags { parse: true }).unwrap();
    assert_eq!(toks.last().unwrap().kind, TokenKind::InlineHtml);
}

#[test]
fn interpolation_tokens() {
    let src = "<?php \"a $b[0] $c->d ${e} {$f['g']} $h[-1] $i[j]\";";
    let names: Vec<&str> = tokenize(src.as_bytes(), TokenizeFlags::default())
        .unwrap()
        .iter()
        .filter(|t| !t.kind.is_char())
        .map(|t| t.kind.token_name())
        .collect();
    assert_eq!(
        names,
        [
            "T_OPEN_TAG",
            "T_ENCAPSED_AND_WHITESPACE",
            "T_VARIABLE",
            "T_NUM_STRING",
            "T_ENCAPSED_AND_WHITESPACE",
            "T_VARIABLE",
            "T_OBJECT_OPERATOR",
            "T_STRING",
            "T_ENCAPSED_AND_WHITESPACE",
            "T_DOLLAR_OPEN_CURLY_BRACES",
            "T_STRING_VARNAME",
            "T_ENCAPSED_AND_WHITESPACE",
            "T_CURLY_OPEN",
            "T_VARIABLE",
            "T_CONSTANT_ENCAPSED_STRING",
            "T_ENCAPSED_AND_WHITESPACE",
            "T_VARIABLE",
            "T_NUM_STRING",
            "T_ENCAPSED_AND_WHITESPACE",
            "T_VARIABLE",
            "T_STRING",
        ]
    );
}
