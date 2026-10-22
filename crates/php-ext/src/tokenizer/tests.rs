//! Unit tests ported from `ext/tokenizer/tests/*.phpt`.
//!
//! Scripts cannot be run yet, so each test re-creates what the `.phpt` script
//! prints and compares it with the file's expected output (embedded in
//! `tests/expected.rs`).

mod expected;

use php_runtime::class::{ClassEntry, PropertyInfo, class_flags, member_flags};
use php_runtime::{CallArgs, ClassRef, Flow, ObjRef, PResult, Value, Vm};

use super::data::{TOKEN_CONSTANTS, get_token_type_name};
use super::*;
use crate::testing::{TestVm, thrown};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn token_get_all(t: &mut TestVm, code: &[u8], flags: i64) -> PResult<Value> {
    t.call("token_get_all", vec![Value::bytes(code), Value::Int(flags)])
}

fn name_of(t: &mut TestVm, id: i64) -> String {
    let v = t.call("token_name", vec![Value::Int(id)]).unwrap();
    v.as_str().unwrap().to_string()
}

/// One element of the `token_get_all()` result.
enum Tok {
    Char(String),
    Token { id: i64, text: Vec<u8>, line: i64 },
}

fn toks(v: &Value) -> Vec<Tok> {
    let Value::Array(a) = v else {
        panic!("not an array: {v:?}")
    };
    a.values()
        .map(|e| match e {
            Value::String(s) => Tok::Char(s.to_string()),
            Value::Array(t) => {
                assert_eq!(t.len(), 3);
                Tok::Token {
                    id: t.get_int(0).unwrap().as_int().unwrap(),
                    text: t.get_int(1).unwrap().as_str().unwrap().to_vec(),
                    line: t.get_int(2).unwrap().as_int().unwrap(),
                }
            }
            other => panic!("unexpected token {other:?}"),
        })
        .collect()
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn get_all(t: &mut TestVm, code: &str, flags: i64) -> Vec<Tok> {
    toks(&token_get_all(t, code.as_bytes(), flags).unwrap())
}

fn phptoken(t: &TestVm) -> ClassRef {
    t.vm.find_class(b"PhpToken").unwrap()
}

/// `$class::tokenize($code, $flags)`.
fn tokenize_static(
    t: &mut TestVm,
    class: &ClassRef,
    code: &str,
    flags: i64,
) -> PResult<Vec<ObjRef>> {
    let method = class.find_method(b"tokenize").unwrap().clone();
    let call = CallArgs {
        positional: vec![Value::str(code), Value::Int(flags)],
        scope: Some(class.clone()),
        ..CallArgs::default()
    };
    let Value::Array(a) = t.vm.call_function(&method, call)? else {
        panic!("tokenize() must return an array")
    };
    Ok(a.values()
        .map(|v| v.as_object().expect("PhpToken object").clone())
        .collect())
}

fn tokenize_objects(t: &mut TestVm, code: &str, flags: i64) -> Vec<ObjRef> {
    let class = phptoken(t);
    tokenize_static(t, &class, code, flags).unwrap()
}

fn prop_int(o: &ObjRef, name: &str) -> i64 {
    o.get_prop(name.as_bytes()).unwrap().as_int().unwrap()
}

fn prop_text(o: &ObjRef) -> String {
    lossy(o.get_prop(b"text").unwrap().as_str().unwrap())
}

fn call_method(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
    t.vm.call_method(o, name.as_bytes(), args)
}

fn token_name_of(t: &mut TestVm, o: &ObjRef) -> String {
    match call_method(t, o, "getTokenName", vec![]).unwrap() {
        Value::String(s) => s.to_string(),
        Value::Null => "NULL".into(),
        other => panic!("{other:?}"),
    }
}

/// `"{$token->getTokenName()}: \"$token->text\"\n"` for every token.
fn name_text_lines(t: &mut TestVm, code: &str, flags: i64) -> String {
    let mut out = String::new();
    for o in tokenize_objects(t, code, flags) {
        let name = token_name_of(t, &o);
        out.push_str(&format!("{name}: \"{}\"\n", prop_text(&o)));
    }
    out
}

fn name_lines(t: &mut TestVm, code: &str, flags: i64) -> String {
    let mut out = String::new();
    for o in tokenize_objects(t, code, flags) {
        out.push_str(&token_name_of(t, &o));
        out.push('\n');
    }
    out
}

fn assert_expected(actual: &str, expected: &str) {
    assert_eq!(actual.trim_end(), expected.trim_end());
}

// ---------------------------------------------------------------------------
// Constants and token_name()
// ---------------------------------------------------------------------------

/// 001.phpt
#[test]
fn token_name_for_constants() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for line in expected::TOKEN_NAMES.lines() {
        match line {
            "UNKNOWN" | "Done" => continue,
            name => {
                let id =
                    t.vm.constant(name)
                        .unwrap_or_else(|| panic!("{name}"))
                        .as_int();
                out.push_str(&name_of(&mut t, id.unwrap()));
                out.push('\n');
            }
        }
    }
    out.push_str(&name_of(&mut t, -1));
    out.push('\n');
    out.push_str(&name_of(&mut t, 0x8000000F));
    out.push_str("\nDone\n");
    assert_expected(&out, expected::TOKEN_NAMES);
}

#[test]
fn constants_are_registered() {
    let t = TestVm::new();
    assert_eq!(t.vm.constant("TOKEN_PARSE").unwrap().as_int(), Some(1));
    assert_eq!(TOKEN_CONSTANTS.len(), 153);
    for name in TOKEN_CONSTANTS {
        let v =
            t.vm.constant(name)
                .unwrap_or_else(|| panic!("{name} missing"));
        assert!(v.as_int().is_some_and(|i| i >= 256), "{name}");
    }
    assert_eq!(
        t.vm.constant("T_DOUBLE_COLON").unwrap().as_int(),
        t.vm.constant("T_PAAMAYIM_NEKUDOTAYIM").unwrap().as_int()
    );
    // Every named grammar token of php-syntax is exported (except T_ERROR).
    for kind in php_syntax::TokenKind::ALL {
        let Some(name) = kind.name().filter(|n| n.starts_with("T_")) else {
            continue;
        };
        if name == "T_ERROR" {
            assert!(t.vm.constant(name).is_none());
            continue;
        }
        assert_eq!(
            t.vm.constant(name)
                .unwrap_or_else(|| panic!("{name} missing"))
                .as_int(),
            Some(kind.id() as i64),
            "{name}"
        );
    }
    // Names round-trip, T_DOUBLE_COLON being the name of T_PAAMAYIM_NEKUDOTAYIM.
    for name in TOKEN_CONSTANTS {
        let id = t.vm.constant(name).unwrap().as_int().unwrap();
        let want = if *name == "T_PAAMAYIM_NEKUDOTAYIM" {
            "T_DOUBLE_COLON"
        } else {
            name
        };
        assert_eq!(get_token_type_name(id as i32), Some(want));
    }
    assert_eq!(get_token_type_name(0), None);
    assert_eq!(get_token_type_name(b';' as i32), None);
    assert_eq!(get_token_type_name(100000), None);
    assert_eq!(get_token_type_name(-5), None);
    // T_ERROR is not a user-visible token.
    assert_eq!(
        get_token_type_name(php_syntax::TokenKind::Error.id() as i32),
        None
    );
}

#[test]
fn token_name_arguments() {
    let mut t = TestVm::new();
    let id = t.vm.constant("T_FUNCTION").unwrap();
    assert_eq!(
        t.call("token_name", vec![id])
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
        "T_FUNCTION"
    );
    // The id is truncated to `int`: 2^32 + id names the same token.
    let big = Value::Int((1 << 32) + 258);
    assert_eq!(
        t.call("token_name", vec![big])
            .unwrap()
            .as_str()
            .unwrap()
            .to_string(),
        "T_THROW"
    );
    let (class, msg) = thrown(t.call("token_name", vec![]));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(msg, "token_name() expects exactly 1 argument, 0 given");
    let (class, msg) = thrown(t.call("token_name", vec![Value::str("abc")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "token_name(): Argument #1 ($id) must be of type int, string given"
    );
}

// ---------------------------------------------------------------------------
// token_get_all()
// ---------------------------------------------------------------------------

/// token_get_all_basic.phpt, 003.phpt
#[test]
fn token_get_all_shapes() {
    let mut t = TestVm::new();
    let id = |t: &TestVm, n: &str| t.vm.constant(n).unwrap().as_int().unwrap();

    let v = get_all(&mut t, "<?php echo \"Hello World\"; ?>", 0);
    let want = [
        ("T_OPEN_TAG", "<?php "),
        ("T_ECHO", "echo"),
        ("T_WHITESPACE", " "),
        ("T_CONSTANT_ENCAPSED_STRING", "\"Hello World\""),
        (";", ";"),
        ("T_WHITESPACE", " "),
        ("T_CLOSE_TAG", "?>"),
    ];
    assert_eq!(v.len(), want.len());
    for (tok, (name, text)) in v.iter().zip(want) {
        match tok {
            Tok::Char(c) => assert_eq!((name, text), (c.as_str(), c.as_str())),
            Tok::Token {
                id: i,
                text: tx,
                line,
            } => {
                assert_eq!(*i, id(&t, name), "{name}");
                assert_eq!(lossy(tx), text);
                assert_eq!(*line, 1);
            }
        }
    }

    // Without open tag: inline HTML.
    let v = get_all(&mut t, "echo 'Hello World';", 0);
    assert_eq!(v.len(), 1);
    let Tok::Token { id: i, text, line } = &v[0] else {
        panic!()
    };
    assert_eq!(*i, id(&t, "T_INLINE_HTML"));
    assert_eq!(lossy(text), "echo 'Hello World';");
    assert_eq!(*line, 1);

    // 003.phpt: empty input and non-string arguments.
    assert!(get_all(&mut t, "", 0).is_empty());
    let r = t.call("token_get_all", vec![Value::Int(0)]).unwrap();
    let v = toks(&r);
    assert!(matches!(&v[..], [Tok::Token { text, line: 1, .. }] if text == b"0"));
    let r = t.call("token_get_all", vec![Value::Int(-1)]).unwrap();
    let v = toks(&r);
    assert!(matches!(&v[..], [Tok::Token { text, line: 1, .. }] if text == b"-1"));
}

/// token_get_all_variation*.phpt style: single-character tokens are strings,
/// everything else is `[id, text, line]`.
#[test]
fn token_get_all_variations() {
    let mut t = TestVm::new();
    let v = get_all(
        &mut t,
        "<?php echo 1; if (isset($a)) print $a+1; $a++; $a--; $a == 2; $a === 2; endif; ?>",
        0,
    );
    assert_eq!(v.len(), 48);
    let describe = |t: &mut TestVm, tok: &Tok| match tok {
        Tok::Char(c) => c.clone(),
        Tok::Token { id, text, .. } => format!("{}({})", name_of(t, *id), lossy(text)),
    };
    let got: Vec<String> = v.iter().map(|x| describe(&mut t, x)).collect();
    assert_eq!(
        got.join(" "),
        "T_OPEN_TAG(<?php ) T_ECHO(echo) T_WHITESPACE( ) T_LNUMBER(1) ; T_WHITESPACE( ) \
         T_IF(if) T_WHITESPACE( ) ( T_ISSET(isset) ( T_VARIABLE($a) ) ) T_WHITESPACE( ) \
         T_PRINT(print) T_WHITESPACE( ) T_VARIABLE($a) + T_LNUMBER(1) ; T_WHITESPACE( ) \
         T_VARIABLE($a) T_INC(++) ; T_WHITESPACE( ) T_VARIABLE($a) T_DEC(--) ; T_WHITESPACE( ) \
         T_VARIABLE($a) T_WHITESPACE( ) T_IS_EQUAL(==) T_WHITESPACE( ) T_LNUMBER(2) ; \
         T_WHITESPACE( ) T_VARIABLE($a) T_WHITESPACE( ) T_IS_IDENTICAL(===) T_WHITESPACE( ) \
         T_LNUMBER(2) ; T_WHITESPACE( ) T_ENDIF(endif) ; T_WHITESPACE( ) T_CLOSE_TAG(?>)"
    );

    let v = get_all(&mut t, "wrong syntax here", 0);
    assert_eq!(v.len(), 1);
    let v = get_all(
        &mut t,
        "<?php /* comment */ $a = 2 | 1; $b = 3^2; $c = 4&2; ?>",
        0,
    );
    let got: Vec<String> = v.iter().map(|x| describe(&mut t, x)).collect();
    assert!(got.contains(&"T_COMMENT(/* comment */)".to_string()));
    assert!(got.contains(&"|".to_string()));
    assert!(got.contains(&"^".to_string()));
    // `&` followed by a number is the "not followed by var" variant.
    assert!(got.contains(&"T_AMPERSAND_NOT_FOLLOWED_BY_VAR_OR_VARARG(&)".to_string()));
}

/// token_get_all_heredoc_nowdoc.phpt
#[test]
fn flexible_heredoc_and_nowdoc() {
    let mut t = TestVm::new();
    let sources: [&str; 18] = [
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
    let mut out = String::new();
    for (title, flags) in [
        ("Without TOKEN_PARSE:", 0),
        ("With TOKEN_PARSE:", TOKEN_PARSE),
    ] {
        if flags != 0 {
            out.push('\n');
        }
        out.push_str(&format!("{title}\n"));
        for (i, src) in sources.iter().enumerate() {
            out.push_str(&format!("\nTest case {}\n\n", i + 1));
            match token_get_all(&mut t, src.as_bytes(), flags) {
                Ok(v) => {
                    for tok in toks(&v) {
                        if let Tok::Token { id, text, line } = tok {
                            let name = name_of(&mut t, id);
                            out.push_str(&format!("Line {line}: {name} ('{}')\n", lossy(&text)));
                        }
                    }
                }
                Err(Flow::Exception(e)) => {
                    assert_eq!(e.class().name, "ParseError");
                    out.push_str(&format!(
                        "ParseError: {} on line {}\n",
                        Vm::exception_message(&e),
                        prop_int(&e, "line")
                    ));
                }
                Err(other) => panic!("{other:?}"),
            }
        }
    }
    assert_expected(&out, expected::HEREDOC_NOWDOC);
}

/// bug26463.phpt, bug60097.phpt, bug76991.phpt
#[test]
fn heredoc_tokens() {
    let mut t = TestVm::new();
    let dump = |t: &mut TestVm, code: &str| -> Vec<String> {
        get_all(t, code, 0)
            .iter()
            .map(|tok| match tok {
                Tok::Char(c) => c.clone(),
                Tok::Token { id, text, line } => {
                    format!("L{line} {} {:?}", name_of(t, *id), lossy(text))
                }
            })
            .collect()
    };
    let got = dump(
        &mut t,
        "<?php\n$x=<<<DD\njhdsjkfhjdsh\nDD\n.\"\";\n$a=<<<DDDD\njhdsjkfhjdsh\nDDDD;\n?>",
    );
    assert_eq!(got.len(), 19);
    assert_eq!(got[3], "L2 T_START_HEREDOC \"<<<DD\\n\"");
    assert_eq!(got[4], "L3 T_ENCAPSED_AND_WHITESPACE \"jhdsjkfhjdsh\\n\"");
    assert_eq!(got[5], "L4 T_END_HEREDOC \"DD\"");
    assert_eq!(got[8], "L5 T_CONSTANT_ENCAPSED_STRING \"\\\"\\\"\"");
    assert_eq!(got[15], "L8 T_END_HEREDOC \"DDDD\"");
    assert_eq!(got[18], "L9 T_CLOSE_TAG \"?>\"");

    let got = dump(&mut t, "<?php\n<<<DOC1\n{$s(<<<DOC2\nDOC2\n)}\nDOC1;\n");
    assert_eq!(got.len(), 14);
    assert_eq!(got[1], "L2 T_START_HEREDOC \"<<<DOC1\\n\"");
    assert_eq!(got[2], "L3 T_CURLY_OPEN \"{\"");
    assert_eq!(got[5], "L3 T_START_HEREDOC \"<<<DOC2\\n\"");
    assert_eq!(got[6], "L4 T_END_HEREDOC \"DOC2\"");
    assert_eq!(got[11], "L6 T_END_HEREDOC \"DOC1\"");

    // bug76991
    let mut out = String::new();
    for tok in get_all(&mut t, "<?php\n<<<TEST\n$a\n TEST;\n\n<<<END\n$a\n END;", 0) {
        match tok {
            Tok::Char(c) => out.push_str(&c),
            Tok::Token { id, text, .. } => {
                out.push_str(&format!(
                    "{}: {}",
                    name_of(&mut t, id),
                    lossy(&text).replace('\n', "\\n")
                ));
            }
        }
        out.push('\n');
    }
    assert_expected(&out, expected::BUG76991);
}

/// bug54089.phpt: `__halt_compiler` and the inline HTML after it.
#[test]
fn halt_compiler() {
    let mut t = TestVm::new();
    let codes = [
        "<?php __halt_compiler",
        "<?php __halt_compiler(",
        "<?php __halt_compiler();",
        "<?php __halt_compiler();ABC",
        "<?php __halt_compiler\n(\n)\n;ABC",
        "<?php __halt_compiler\nabc\ndef\nghi ABC",
    ];
    let counts = [2, 3, 5, 6, 9, 9];
    for (code, count) in codes.iter().zip(counts) {
        let v = get_all(&mut t, code, 0);
        assert_eq!(v.len(), count, "{code:?}");
        // The tokens always add up to the original source.
        let joined: String = v
            .iter()
            .map(|tok| match tok {
                Tok::Char(c) => c.clone(),
                Tok::Token { text, .. } => lossy(text),
            })
            .collect();
        assert_eq!(&joined, code);
    }
    // The last token after `;` is inline HTML.
    let v = get_all(&mut t, "<?php __halt_compiler();ABC", 0);
    let Tok::Token { id, text, .. } = &v[5] else {
        panic!()
    };
    assert_eq!(name_of(&mut t, *id), "T_INLINE_HTML");
    assert_eq!(text, b"ABC");
    let v = get_all(&mut t, "<?php __halt_compiler\nabc\ndef\nghi ABC", 0);
    let Tok::Token { id, text, .. } = v.last().unwrap() else {
        panic!()
    };
    assert_eq!(name_of(&mut t, *id), "T_INLINE_HTML");
    assert_eq!(text, b" ABC");
}

/// attributes.phpt, bug67395.phpt, invalid_octal_dnumber.phpt,
/// invalid_large_octal_with_underscores.phpt, no_inline_html_split.phpt, php_tag_only.phpt
#[test]
fn assorted_token_kinds() {
    let mut t = TestVm::new();
    let first_ids = |t: &mut TestVm, code: &str, flags: i64| -> Vec<String> {
        get_all(t, code, flags)
            .iter()
            .map(|tok| match tok {
                Tok::Char(c) => c.clone(),
                Tok::Token { id, .. } => name_of(t, *id),
            })
            .collect()
    };
    // attributes
    let v = get_all(&mut t, "<?php #[A1(1, 2)] class C1 { }", 0);
    let Tok::Token { id, text, .. } = &v[1] else {
        panic!()
    };
    assert_eq!(name_of(&mut t, *id), "T_ATTRIBUTE");
    assert_eq!(text, b"#[");
    assert!(matches!(&v[2], Tok::Token { text, .. } if text == b"A1"));
    // bug67395
    assert_eq!(first_ids(&mut t, "<?php **", 0)[1], "T_POW");
    assert_eq!(first_ids(&mut t, "<?php **=", 0)[1], "T_POW_EQUAL");
    // Invalid octal turns into a float literal.
    assert_eq!(
        first_ids(&mut t, "<?php 0177777777777777777777787", 0)[1],
        "T_DNUMBER"
    );
    let v = get_all(&mut t, "<?php 0_10000000000000000000009;", 0);
    assert_eq!(v.len(), 3);
    assert!(matches!(&v[1], Tok::Token { text, .. } if text == b"0_10000000000000000000009"));
    // `<?php` without a trailing space is an open tag too.
    assert_eq!(first_ids(&mut t, "<?php", 0), ["T_OPEN_TAG"]);
    assert_eq!(
        first_ids(&mut t, "Foobar<?php", 0),
        ["T_INLINE_HTML", "T_OPEN_TAG"]
    );
    // Inline HTML is not split by something that is not an open tag.
    let v = get_all(&mut t, "Foo<?phpBar", 0);
    assert_eq!(v.len(), 1);
    assert!(matches!(&v[0], Tok::Token { text, .. } if text == b"Foo<?phpBar"));
}

/// bad_character.phpt
#[test]
fn bad_characters() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for code in [
        &b"<?php \0 foo"[..],
        b"<?php \x01 bar",
        b"<?php \x01\x02 bar \x03",
    ] {
        for tok in toks(&token_get_all(&mut t, code, 0).unwrap()) {
            match tok {
                Tok::Char(c) => out.push_str(&c),
                Tok::Token { id, text, .. } => {
                    out.push_str(&format!("{} {}", name_of(&mut t, id), text.len()));
                }
            }
            out.push('\n');
        }
        out.push('\n');
    }
    assert_expected(&out, expected::BAD_CHARACTER);
}

/// bug76437.phpt, TOKEN_PARSE gives the same open/close tags.
#[test]
fn open_and_close_tag_tokens() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for (code, index) in [
        ("<?=$a?>", 0),
        ("<?php echo 2; ?>", 6),
        ("<?php echo 2; ?>\n", 6),
    ] {
        let plain = token_get_all(&mut t, code.as_bytes(), 0).unwrap();
        let parsed = token_get_all(&mut t, code.as_bytes(), TOKEN_PARSE).unwrap();
        let Tok::Token { id, text, line } = toks(&plain).swap_remove(index) else {
            panic!()
        };
        out.push_str(&format!(
            "{}: \"{}\" on line {line}\n",
            name_of(&mut t, id),
            lossy(&text)
        ));
        let same = format!("{:?}", plain) == format!("{:?}", parsed);
        out.push_str(&format!("bool({same})\n"));
    }
    assert_expected(&out, expected::BUG76437);
}

// ---------------------------------------------------------------------------
// TOKEN_PARSE
// ---------------------------------------------------------------------------

/// token_get_all_TOKEN_PARSE_000.phpt
#[test]
fn token_parse_error() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(token_get_all(&mut t, b"<?php invalid code;", TOKEN_PARSE));
    assert_eq!(class, "ParseError");
    assert_eq!(msg, "syntax error, unexpected identifier \"code\"");
    // The tokens without TOKEN_PARSE are fine.
    assert!(token_get_all(&mut t, b"<?php invalid code;", 0).is_ok());
    // The exception reports the compiled file name (empty) and the error line.
    let Err(Flow::Exception(e)) = token_get_all(&mut t, b"<?php\n\ninvalid code;", TOKEN_PARSE)
    else {
        panic!()
    };
    assert_eq!(prop_int(&e, "line"), 3);
    assert_eq!(e.get_prop(b"file").unwrap().as_str().unwrap().len(), 0);
}

/// parse_errors.phpt
#[test]
fn token_parse_lexer_errors() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for code in [
        "<?php var_dump(078);",
        "<?php var_dump(\"\\u{xyz}\");",
        "<?php var_dump(\"\\u{ffffff}\");",
        "<?php var_dump(078 + 078);",
    ] {
        match token_get_all(&mut t, code.as_bytes(), TOKEN_PARSE) {
            Err(Flow::Exception(e)) => {
                out.push_str(&format!(
                    "{}: {}\n",
                    e.class().name,
                    Vm::exception_message(&e)
                ));
            }
            other => panic!("expected ParseError, got {other:?}"),
        }
        for tok in toks(&token_get_all(&mut t, code.as_bytes(), 0).unwrap()) {
            match tok {
                Tok::Char(c) => out.push_str(&c),
                Tok::Token { id, text, .. } => {
                    out.push_str(&format!("{} ({})", name_of(&mut t, id), lossy(&text)));
                }
            }
            out.push('\n');
        }
        out.push('\n');
    }
    assert_expected(&out, expected::PARSE_ERRORS);
}

fn dump_lines(t: &mut TestVm, code: &str, flags: i64) -> String {
    let mut out = String::new();
    for tok in get_all(t, code, flags) {
        match tok {
            Tok::Char(c) => out.push_str(&c),
            Tok::Token { id, text, line } => {
                let name = name_of(t, id);
                if name == "T_WHITESPACE" {
                    continue;
                }
                out.push_str(&format!("L{line}: {name} {}", lossy(&text)));
            }
        }
        out.push('\n');
    }
    out.push_str("Done\n");
    out
}

/// token_get_all_TOKEN_PARSE_001.phpt, token_get_all_TOKEN_PARSE_002.phpt:
/// semi-reserved words are `T_STRING`s after `::` and `->`.
#[test]
fn token_parse_semi_reserved_words() {
    let mut t = TestVm::new();
    let code = "<?php\nX::continue;\nX::$continue;\n$x->$continue;\nX::continue();\n$x->continue();\nX::class;\n$x->__halt_compiler();\n\nclass X {\n    const CONTINUE = 1;\n    public $x = self::CONTINUE + 1;\n}\n";
    let out = dump_lines(&mut t, code, TOKEN_PARSE);
    assert_expected(&out, expected::SEMI_RESERVED_MEMBER);

    let code = "<?php\n  class SomeClass {\n      const CONST = 1;\n      const CONTINUE = (self::CONST + 1);\n      const ARRAY = [1, self::CONTINUE => [3, 4], 5];\n  }\n";
    let out = dump_lines(&mut t, code, TOKEN_PARSE);
    assert_expected(&out, expected::SEMI_RESERVED_CONST);

    // Without TOKEN_PARSE the same words keep their keyword ids.
    let plain = dump_lines(&mut t, "<?php X::continue;", 0);
    assert!(plain.contains("T_CONTINUE continue"), "{plain}");
}

/// bug77966.phpt, bug80462.phpt, bug81342.phpt, namespaced_names.phpt
#[test]
fn phptoken_name_listings() {
    let mut t = TestVm::new();
    let out = name_text_lines(
        &mut t,
        "<?php\nclass C {\n    use A {\n        namespace as bar;\n    }\n}",
        TOKEN_PARSE,
    );
    assert_expected(&out, expected::BUG77966);

    let out = name_lines(&mut t, "<?php $foo = $a?->b();", TOKEN_PARSE);
    assert_expected(&out, expected::BUG80462);

    let out = name_lines(&mut t, "<?php $x & $x; $x &\n    $baz;\n", 0);
    assert_expected(&out, expected::BUG81342);

    let out = name_text_lines(
        &mut t,
        "<?php\nFoo\nFoo\\Bar\n\\Foo\\Bar\nnamespace\\Foo\nFoo \\ Bar",
        0,
    );
    assert_expected(&out, expected::NAMESPACED_NAMES);
}

/// gh19507_*.phpt: deprecations raised while parsing reach the error handler
/// (here: the displayed message) before the tokens are returned.
#[test]
fn token_parse_diagnostics() {
    let mut t = TestVm::new();
    let v = token_get_all(&mut t, b"<?php (double) $x;", TOKEN_PARSE).unwrap();
    assert_eq!(toks(&v).len(), 5);
    assert_eq!(
        t.output(),
        "\nDeprecated: Non-canonical cast (double) is deprecated, use the (float) cast instead in  on line 1\n"
    );
    // Without TOKEN_PARSE, nothing is raised.
    token_get_all(&mut t, b"<?php (double) $x;", 0).unwrap();
    assert_eq!(t.output(), "");
}

// ---------------------------------------------------------------------------
// PhpToken
// ---------------------------------------------------------------------------

/// PhpToken_constructor.phpt
#[test]
fn phptoken_constructor() {
    let mut t = TestVm::new();
    let class = phptoken(&t);
    let new = |t: &mut TestVm, args: Vec<Value>| {
        let o = t.vm.instantiate(&class).unwrap();
        call_method(t, &o, "__construct", args).unwrap();
        o
    };
    let o = new(&mut t, vec![Value::Int(300), Value::str("function")]);
    assert_eq!(prop_int(&o, "id"), 300);
    assert_eq!(prop_text(&o), "function");
    assert_eq!(prop_int(&o, "line"), -1);
    assert_eq!(prop_int(&o, "pos"), -1);
    let o = new(
        &mut t,
        vec![Value::Int(300), Value::str("function"), Value::Int(10)],
    );
    assert_eq!((prop_int(&o, "line"), prop_int(&o, "pos")), (10, -1));
    let o = new(
        &mut t,
        vec![
            Value::Int(300),
            Value::str("function"),
            Value::Int(10),
            Value::Int(100),
        ],
    );
    assert_eq!((prop_int(&o, "line"), prop_int(&o, "pos")), (10, 100));
    // Property order: id, text, line, pos.
    let names: Vec<String> = o.properties().iter().map(|(k, _)| k.to_string()).collect();
    assert_eq!(names, ["id", "text", "line", "pos"]);

    let (class_name, msg) =
        thrown(t.vm.call_method(&o, b"__construct", vec![Value::str("x"), Value::str("y")]));
    assert_eq!(class_name, "TypeError");
    assert_eq!(
        msg,
        "PhpToken::__construct(): Argument #1 ($id) must be of type int, string given"
    );
}

/// The class declaration: properties, interface and final constructor
/// (PhpToken_final_constructor.phpt, PhpToken_toString.phpt).
#[test]
fn phptoken_class_shape() {
    let t = TestVm::new();
    let class = phptoken(&t);
    assert!(class.is_internal());
    assert!(class.instance_of_name(b"Stringable"));
    for (name, ty) in [
        ("id", "int"),
        ("text", "string"),
        ("line", "int"),
        ("pos", "int"),
    ] {
        let info = class.find_property(name.as_bytes()).unwrap();
        assert_eq!(info.visibility(), member_flags::PUBLIC);
        assert_eq!(info.ty.as_ref().unwrap().to_type_string(), ty);
        assert!(info.default.is_undef());
    }
    let ctor = class.find_method(b"__construct").unwrap();
    assert!(ctor.has_flag(member_flags::FINAL));
    let tokenize = class.find_method(b"tokenize").unwrap();
    assert!(tokenize.is_static());
    assert_eq!(
        class
            .find_method(b"getTokenName")
            .unwrap()
            .signature()
            .return_type
            .as_ref()
            .unwrap()
            .to_type_string(),
        "?string"
    );
    // A new instance has uninitialised typed properties.
    assert!(ObjRef::new(class).properties().is_empty());
}

/// PhpToken_tokenize.phpt
#[test]
fn phptoken_tokenize_positions() {
    let mut t = TestVm::new();
    let code = "<?php\nfunction foo() {\n    echo \"bar\";\n}";
    let expect = [
        ("<?php\n", 1, 0),
        ("function", 2, 6),
        (" ", 2, 14),
        ("foo", 2, 15),
        ("(", 2, 18),
        (")", 2, 19),
        (" ", 2, 20),
        ("{", 2, 21),
        ("\n    ", 2, 22),
        ("echo", 3, 27),
        (" ", 3, 31),
        ("\"bar\"", 3, 32),
        (";", 3, 37),
        ("\n", 3, 38),
        ("}", 4, 39),
    ];
    for flags in [0, TOKEN_PARSE] {
        let tokens = tokenize_objects(&mut t, code, flags);
        assert_eq!(tokens.len(), expect.len());
        for (tok, (text, line, pos)) in tokens.iter().zip(expect) {
            assert_eq!(prop_text(tok), text);
            assert_eq!(
                (prop_int(tok, "line"), prop_int(tok, "pos")),
                (line, pos),
                "{text:?}"
            );
        }
        // Handles follow the object store: fresh ids ascend, and ids freed by the
        // previous run are reused most recently freed first (descending).
        if flags == 0 {
            assert!(tokens.windows(2).all(|w| w[1].id() == w[0].id() + 1));
        } else {
            assert!(tokens.windows(2).all(|w| w[1].id() + 1 == w[0].id()));
        }
        // Characters are tokens too, with their code point as id.
        assert_eq!(prop_int(&tokens[4], "id"), b'(' as i64);
    }
}

/// PhpToken_toString.phpt
#[test]
fn phptoken_to_string() {
    let mut t = TestVm::new();
    let code = "<?php echo \"Hello \". $what;";
    let tokens = tokenize_objects(&mut t, code, 0);
    let joined: String = tokens.iter().map(prop_text).collect();
    assert_eq!(joined, code);
    let s = call_method(&mut t, &tokens[0], "__toString", vec![]).unwrap();
    assert_eq!(s.as_str().unwrap().to_string(), "<?php ");
    // Stringable: the object casts to its text.
    let cast =
        t.vm.to_php_string(&Value::Object(tokens[0].clone()))
            .unwrap();
    assert_eq!(cast.to_string(), "<?php ");
}

/// PhpToken_methods.phpt
#[test]
fn phptoken_methods() {
    let mut t = TestVm::new();
    let code = "<?php\n// comment\n/** comment */\nfunction foo() {\n    echo \"bar\";\n}";
    let tokens = tokenize_objects(&mut t, code, 0);
    let mut out = String::new();
    for (i, token) in tokens.iter().enumerate() {
        let name = token_name_of(&mut t, token);
        let ignorable = call_method(&mut t, token, "isIgnorable", vec![]).unwrap();
        out.push_str(&format!(
            "[{i:>2}] {name:<26} {}\n",
            if ignorable.as_bool().unwrap() {
                "ignorable"
            } else {
                "meaningful"
            }
        ));
    }

    let id = |t: &TestVm, n: &str| t.vm.constant(n).unwrap();
    let is = |t: &mut TestVm, token: &ObjRef, kind: Value| -> PResult<bool> {
        Ok(call_method(t, token, "is", vec![kind])?.as_bool().unwrap())
    };
    let list = |items: Vec<Value>| Value::array(php_runtime::PhpArray::from_values(items));
    let token = tokens[5].clone();
    let b = |v: bool| format!("bool({v})\n");

    out.push_str("\nSuccess:\n");
    let cases = [
        id(&t, "T_FUNCTION"),
        Value::str("function"),
        list(vec![Value::str("class"), id(&t, "T_FUNCTION")]),
        list(vec![id(&t, "T_CLASS"), Value::str("function")]),
    ];
    for c in cases {
        out.push_str(&b(is(&mut t, &token, c).unwrap()));
    }
    out.push_str("\nFailure:\n");
    let cases = [
        id(&t, "T_CLASS"),
        Value::str("class"),
        list(vec![Value::str("class"), id(&t, "T_TRAIT")]),
        list(vec![id(&t, "T_CLASS"), Value::str("trait")]),
    ];
    for c in cases {
        out.push_str(&b(is(&mut t, &token, c).unwrap()));
    }

    out.push_str("\nError:\n");
    let err = |t: &mut TestVm, r: PResult<bool>| {
        let _ = t;
        match r {
            Err(Flow::Exception(e)) => {
                format!("{}: {}\n", e.class().name, Vm::exception_message(&e))
            }
            other => panic!("expected exception, got {other:?}"),
        }
    };
    let r = is(&mut t, &token, Value::Float(2.5));
    out.push_str(&err(&mut t, r));
    let r = is(&mut t, &token, list(vec![Value::Float(2.5)]));
    out.push_str(&err(&mut t, r));

    {
        let mut props = token.obj().props.borrow_mut();
        props[0] = Value::Undef;
        props[1] = Value::Undef;
    }
    for kind in [
        id(&t, "T_FUNCTION"),
        Value::str("function"),
        list(vec![id(&t, "T_FUNCTION")]),
        list(vec![Value::str("function")]),
    ] {
        let r = is(&mut t, &token, kind);
        out.push_str(&err(&mut t, r));
    }

    out.push_str("\nName of unknown token:\n");
    let class = phptoken(&t);
    let unknown = t.vm.instantiate(&class).unwrap();
    call_method(
        &mut t,
        &unknown,
        "__construct",
        vec![Value::Int(100000), Value::str("foo")],
    )
    .unwrap();
    out.push_str(&format!("{}\n", token_name_of(&mut t, &unknown)));

    assert_expected(&out, expected::PHPTOKEN_METHODS);
}

#[test]
fn phptoken_get_token_name_edge_cases() {
    let mut t = TestVm::new();
    let class = phptoken(&t);
    let make = |t: &mut TestVm, id: i64| {
        let o = t.vm.instantiate(&class).unwrap();
        call_method(t, &o, "__construct", vec![Value::Int(id), Value::str("x")]).unwrap();
        o
    };
    // Ids below 256 are the character itself (truncated to a byte).
    let o = make(&mut t, b';' as i64);
    assert_eq!(token_name_of(&mut t, &o), ";");
    let o = make(&mut t, -1);
    let v = call_method(&mut t, &o, "getTokenName", vec![]).unwrap();
    assert_eq!(v.as_str().unwrap().as_bytes(), b"\xff");
    let double_colon =
        t.vm.constant("T_PAAMAYIM_NEKUDOTAYIM")
            .unwrap()
            .as_int()
            .unwrap();
    let o = make(&mut t, double_colon);
    assert_eq!(token_name_of(&mut t, &o), "T_DOUBLE_COLON");
    // Not ignorable unless whitespace, comment, doc comment or open tag.
    for (name, want) in [
        ("T_WHITESPACE", true),
        ("T_COMMENT", true),
        ("T_DOC_COMMENT", true),
        ("T_OPEN_TAG", true),
        ("T_OPEN_TAG_WITH_ECHO", false),
        ("T_CLOSE_TAG", false),
        ("T_STRING", false),
    ] {
        let id = t.vm.constant(name).unwrap().as_int().unwrap();
        let o = make(&mut t, id);
        let v = call_method(&mut t, &o, "isIgnorable", vec![]).unwrap();
        assert_eq!(v.as_bool(), Some(want), "{name}");
    }
    // `is()` with an unsupported type.
    let (class_name, msg) = thrown(call_method(&mut t, &o, "is", vec![Value::Null]));
    assert_eq!(class_name, "TypeError");
    assert_eq!(
        msg,
        "PhpToken::is(): Argument #1 ($kind) must be of type string|int|array, null given"
    );
    let (_, msg) = thrown(call_method(&mut t, &o, "is", vec![Value::Bool(true)]));
    assert!(
        msg.ends_with("must be of type string|int|array, true given"),
        "{msg}"
    );
}

fn declare_subclass(t: &mut TestVm, name: &str, flags: u32, extra: bool) -> ClassRef {
    let parent = phptoken(t);
    let mut ce = ClassEntry::new_subclass(name, flags, &parent);
    if extra {
        let mut info = PropertyInfo::new("extra", name, member_flags::PUBLIC);
        info.default = Value::Int(123);
        ce.declare_property(info);
    }
    php_runtime::Registry::new(&mut t.vm, "tokenizer_tests").class(ce)
}

/// PhpToken_extension.phpt, PhpToken_extension_errors.phpt
#[test]
fn phptoken_subclasses() {
    let mut t = TestVm::new();
    let class = declare_subclass(&mut t, "MyPhpToken", 0, true);
    let tokens = tokenize_static(
        &mut t,
        &class,
        "<?PHP\nFUNCTION FOO() {\n    ECHO \"bar\";\n}",
        0,
    )
    .unwrap();
    let mut out = String::new();
    for tok in &tokens {
        assert_eq!(tok.class().name, "MyPhpToken");
        out.push_str(&prop_text(tok).to_lowercase());
        assert_eq!(
            prop_int(tok, "extra"),
            123,
            "default value of the extra property"
        );
    }
    assert_eq!(out, "<?php\nfunction foo() {\n    echo \"bar\";\n}");

    // Abstract subclasses cannot be instantiated.
    let abstract_class = declare_subclass(&mut t, "MyPhpToken2", class_flags::ABSTRACT, false);
    let (class_name, msg) =
        thrown(tokenize_static(&mut t, &abstract_class, "<?php foo", 0).map(|_| Value::Null));
    assert_eq!(class_name, "Error");
    assert_eq!(msg, "Cannot instantiate abstract class MyPhpToken2");

    // Without a scope the base class is used.
    let class = phptoken(&t);
    assert_eq!(
        tokenize_static(&mut t, &class, "<?php foo", 0)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn phptoken_tokenize_parse_errors() {
    let mut t = TestVm::new();
    let class = phptoken(&t);
    let r = tokenize_static(&mut t, &class, "<?php invalid code;", TOKEN_PARSE);
    let (class_name, msg) = thrown(r.map(|_| Value::Null));
    assert_eq!(class_name, "ParseError");
    assert_eq!(msg, "syntax error, unexpected identifier \"code\"");
}

#[test]
fn token_get_all_argument_errors() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call("token_get_all", vec![]));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(msg, "token_get_all() expects at least 1 argument, 0 given");
    let (class, msg) = thrown(t.call("token_get_all", vec![Value::empty_array()]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "token_get_all(): Argument #1 ($code) must be of type string, array given"
    );
    // Unknown flag bits are ignored.
    assert!(
        t.call("token_get_all", vec![Value::str("<?php 1;"), Value::Int(2)])
            .is_ok()
    );
}
