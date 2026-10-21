//! Parse error messages and lines, compared with php-src's output.

use php_syntax::{ParseErrorKind, parse};

fn err(src: &str) -> (String, u32) {
    let e = parse(src.as_bytes(), "t.php").expect_err(src);
    assert_eq!(e.kind, ParseErrorKind::Parse, "{src}: {}", e.message);
    (e.message, e.line)
}

#[track_caller]
fn check(src: &str, msg: &str, line: u32) {
    assert_eq!(err(src), (msg.to_string(), line), "{src}");
}

#[test]
fn syntax_errors_match_bison() {
    check("<?php\nif ($a)", "syntax error, unexpected end of file", 2);
    check("<?php\nfoo(", "Unclosed '('", 2);
    check(
        "<?php $a = 1\n$b = 2;",
        "syntax error, unexpected variable \"$b\"",
        2,
    );
    check(
        "<?php class A { private }",
        "syntax error, unexpected token \"}\", expecting variable",
        1,
    );
    check(
        "<?php function exit() {}",
        "syntax error, unexpected token \"exit\", expecting \"(\"",
        1,
    );
    check(
        "<?php const exit = 1;",
        "syntax error, unexpected token \"exit\", expecting identifier",
        1,
    );
    check("<?php foo(,);", "syntax error, unexpected token \",\"", 1);
    check(
        "<?php foo(1,,2);",
        "syntax error, unexpected token \",\", expecting \")\"",
        1,
    );
    check("<?php 1 2;", "syntax error, unexpected integer \"2\"", 1);
    check(
        "<?php $a = \"x\" \"y\";",
        "syntax error, unexpected double-quoted string \"y\"",
        1,
    );
    check(
        "<?php $a = 'x' 'y';",
        "syntax error, unexpected single-quoted string \"y\"",
        1,
    );
    check(
        "<?php echo 1 == 2 == 3;",
        "syntax error, unexpected token \"==\"",
        1,
    );
    check(
        "<?php abstract trait T {}",
        "syntax error, unexpected token \"trait\", expecting \"abstract\" or \"final\" or \"readonly\" or \"class\"",
        1,
    );
    check(
        "<?php use A, \\B as C, ;",
        "syntax error, unexpected token \";\", expecting identifier or fully qualified name or namespaced name",
        1,
    );
    check(
        "<?php $x = $a->;",
        "syntax error, unexpected token \";\", expecting identifier or variable or \"{\" or \"$\"",
        1,
    );
    check(
        "<?php echo \"a\" . ;",
        "syntax error, unexpected token \";\"",
        1,
    );
    check(
        "<?php $a = <<<EOT\nfoo\nEOT",
        "syntax error, unexpected end of file, expecting variable or heredoc end or \"${\" or \"{$\"",
        3,
    );
    check("<?php \"$a", "syntax error, unexpected end of file", 1);
    check(
        "<?php $a\x7f;",
        "syntax error, unexpected character 0x7F",
        1,
    );
    check(
        "<?php echo 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' 1;",
        "syntax error, unexpected integer \"1\", expecting \",\" or \";\"",
        1,
    );
    check(
        "<?php foo() bar;",
        "syntax error, unexpected identifier \"bar\"",
        1,
    );
    check(
        "<?php if ($a) { ?>x<?php } else ?>y <?php }",
        "Unmatched '}'",
        1,
    );
    check(
        "<?php if ($a) { ?>x<?php } else ?>y <?php ]",
        "Unmatched ']'",
        1,
    );
    check(
        "<?php if ($a) { ?>x<?php } else ?>y <?php :",
        "syntax error, unexpected token \":\", expecting end of file",
        1,
    );
    check(
        "<?php \\",
        "syntax error, unexpected token \"\\\", expecting end of file",
        1,
    );
    check(
        "<?php function f(A & B) {}",
        "syntax error, unexpected token \")\", expecting variable",
        1,
    );
    check(
        "<?php fn() => &$x;",
        "syntax error, unexpected token \"&\"",
        1,
    );
}

#[test]
fn lexer_errors() {
    check("<?php 078;", "Invalid numeric literal", 1);
    check(
        "<?php\n/* unterminated",
        "Unterminated comment starting line 2",
        2,
    );
    check(
        "<?php \"\\u{110000}\";",
        "Invalid UTF-8 codepoint escape sequence: Codepoint too large",
        1,
    );
    check(
        "<?php \"\\u{zz}\";",
        "Invalid UTF-8 codepoint escape sequence",
        1,
    );
    check("<?php\n{\n", "Unclosed '{' on line 2", 3);
    check("<?php foo(];", "Unclosed '(' does not match ']'", 1);
    check("<?php );", "Unmatched ')'", 1);
    check(
        "<?php $a = <<<EOT\n  a\n b\n  EOT;",
        "Invalid body indentation level (expecting an indentation level of at least 2)",
        3,
    );
    check(
        "<?php $a = <<<EOT\n \ta\n \tEOT;",
        "Invalid indentation - tabs and spaces cannot be mixed",
        2,
    );
    check(
        "<?php $a = <<<EOT\n$b\n  EOT;",
        "Invalid body indentation level (expecting an indentation level of at least 2)",
        2,
    );
    check(
        "<?php trait T {} class C { use T { x as y?><?= as e; } }",
        "Cannot use \"<?=\" as an identifier",
        1,
    );
}

#[test]
fn lexer_error_after_syntax_error_is_not_reported() {
    // bison stops at the first syntax error before the lexer reaches 078.
    check(
        "<?php 1 2; 078;",
        "syntax error, unexpected integer \"2\"",
        1,
    );
    // ...but a lexer error at the lookahead wins.
    check("<?php $a = 078;", "Invalid numeric literal", 1);
}

#[test]
fn compile_errors_from_resolution() {
    let cases = [
        (
            "<?php class Foo {} use Bar\\Foo;",
            "Cannot use Bar\\Foo as Foo because the name is already in use",
        ),
        (
            "<?php use A\\B; use C\\B;",
            "Cannot use C\\B as B because the name is already in use",
        ),
        (
            "<?php use function A\\f; use function B\\f;",
            "Cannot use function B\\f as f because the name is already in use",
        ),
        (
            "<?php use Foo as self;",
            "Cannot use Foo as self because 'self' is a special class name",
        ),
        (
            "<?php echo 1; namespace A;",
            "Namespace declaration statement has to be the very first statement or after any declare call in the script",
        ),
        (
            "<?php namespace A; namespace B { }",
            "Cannot mix bracketed namespace declarations with unbracketed namespace declarations",
        ),
        (
            "<?php namespace A { } echo 1;",
            "No code may exist outside of namespace {}",
        ),
        (
            "<?php namespace A { namespace B { } }",
            "Namespace declarations cannot be nested",
        ),
        (
            "<?php namespace namespace;",
            "Cannot use 'namespace' as namespace name",
        ),
        ("<?php new \\self;", "'\\self' is an invalid class name"),
        (
            "<?php class int {}",
            "Cannot use \"int\" as a class name as it is reserved",
        ),
        (
            "<?php class A extends parent {}",
            "Cannot use \"parent\" as class name, as it is reserved",
        ),
        (
            "<?php function f(\\int $x) {}",
            "Type declaration 'int' must be unqualified",
        ),
        (
            "<?php function f(A\\int $x) {}",
            "Cannot use \"A\\int\" as a type name as it is reserved",
        ),
        (
            "<?php use function A\\f; function f() {}",
            "Cannot redeclare function f() (previously declared as local import)",
        ),
        (
            "<?php use A\\C; class C {}",
            "Cannot redeclare class C (previously declared as local import)",
        ),
        (
            "<?php use const A\\X; const X = 1;",
            "Cannot declare const X because the name is already in use",
        ),
        (
            "<?php function assert() {}",
            "Defining a custom assert() function is not allowed, as the function has special semantics",
        ),
    ];
    for (src, msg) in cases {
        let e = parse(src.as_bytes(), "t").expect_err(src);
        if msg == "syntax error" {
            assert!(e.message.starts_with(msg), "{src}: {}", e.message);
            continue;
        }
        assert_eq!(e.kind, ParseErrorKind::Compile, "{src}");
        assert_eq!(e.message, msg, "{src}");
    }
}

#[test]
fn diagnostics_before_error_are_kept() {
    let e = parse(b"<?php $a = (integer)$b; 1 2;", "t").unwrap_err();
    assert_eq!(e.diagnostics.len(), 1);
    assert!(
        e.diagnostics[0]
            .message
            .starts_with("Non-canonical cast (integer)")
    );
}
