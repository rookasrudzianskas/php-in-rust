//! Name resolution (`zend_resolve_*_name`).

use php_syntax::ast::*;
use php_syntax::parse;

/// Collect `(kind, as written, resolved, fallback)` for every call, constant
/// and `new` in expression statements.
fn names(src: &str) -> Vec<(&'static str, String, String, bool)> {
    let p = parse(src.as_bytes(), "t").unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::new();
    fn walk(stmts: &[Stmt], out: &mut Vec<(&'static str, String, String, bool)>) {
        for s in stmts {
            match &s.kind {
                StmtKind::Expr(e) => expr(e, out),
                StmtKind::Namespace { body: Some(b), .. } => walk(b, out),
                _ => {}
            }
        }
    }
    fn expr(e: &Expr, out: &mut Vec<(&'static str, String, String, bool)>) {
        let mut push =
            |k, n: &Name| out.push((k, n.source_text(), n.resolved.to_string(), n.fallback));
        match &e.kind {
            ExprKind::Call {
                callee: Callee::Name(n),
                ..
            } => push("fn", n),
            ExprKind::Const(n) => push("const", n),
            ExprKind::New {
                class: NewClass::Name(n),
                ..
            } => push("class", n),
            ExprKind::ClassConst {
                class: ClassRef::Name(n),
                ..
            } => push("class", n),
            _ => {}
        }
    }
    walk(&p.stmts, &mut out);
    out
}

fn n(
    k: &'static str,
    src: &str,
    res: &str,
    fallback: bool,
) -> (&'static str, String, String, bool) {
    (k, src.to_string(), res.to_string(), fallback)
}

#[test]
fn global_namespace() {
    assert_eq!(
        names("<?php foo(); FOO; new Foo; \\A\\b(); A\\b();"),
        [
            n("fn", "foo", "foo", false),
            n("const", "FOO", "FOO", false),
            n("class", "Foo", "Foo", false),
            n("fn", "\\A\\b", "A\\b", false),
            n("fn", "A\\b", "A\\b", false),
        ]
    );
}

#[test]
fn namespaced_with_imports() {
    let src = r"<?php
namespace App;
use Lib\Http, Lib\Model as M;
use function Lib\helper;
use const Lib\VERSION;
foo(); helper(); FOO; VERSION; true; NULL;
new Bar; new M; new Http\Client; new \Exact; new namespace\Rel; new self; new static;
M\X::Y; strlen('x');
";
    assert_eq!(
        names(src),
        [
            n("fn", "foo", "App\\foo", true),
            n("fn", "helper", "Lib\\helper", false),
            n("const", "FOO", "App\\FOO", true),
            n("const", "VERSION", "Lib\\VERSION", false),
            n("const", "true", "true", false),
            n("const", "NULL", "NULL", false),
            n("class", "Bar", "App\\Bar", false),
            n("class", "M", "Lib\\Model", false),
            n("class", "Http\\Client", "Lib\\Http\\Client", false),
            n("class", "\\Exact", "Exact", false),
            n("class", "namespace\\Rel", "App\\Rel", false),
            n("class", "self", "self", false),
            n("class", "static", "static", false),
            n("class", "M\\X", "Lib\\Model\\X", false),
            n("fn", "strlen", "App\\strlen", true),
        ]
    );
}

#[test]
fn imports_are_case_insensitive_except_constants() {
    let src = r"<?php
namespace N;
use Lib\Thing;
use const Lib\LIMIT;
new THING; LIMIT; limit;
";
    assert_eq!(
        names(src),
        [
            n("class", "THING", "Lib\\Thing", false),
            n("const", "LIMIT", "Lib\\LIMIT", false),
            n("const", "limit", "N\\limit", true),
        ]
    );
}

#[test]
fn group_use_and_namespace_reset() {
    let src = r"<?php
namespace A {
    use X\{Y, function z, const W};
    new Y; z(); W;
}
namespace B {
    new Y;
}
namespace {
    new Y;
}
";
    assert_eq!(
        names(src),
        [
            n("class", "Y", "X\\Y", false),
            n("fn", "z", "X\\z", false),
            n("const", "W", "X\\W", false),
            n("class", "Y", "B\\Y", false),
            n("class", "Y", "Y", false),
        ]
    );
}

#[test]
fn declarations_get_namespaced_names_and_use_warnings() {
    let p = parse(b"<?php use Foo; namespace\\f();", "t").unwrap();
    assert_eq!(
        p.diagnostics[0].message,
        "The use statement with non-compound name 'Foo' has no effect"
    );
    let p = parse(
        b"<?php namespace A\\B; function f() {} class C {} const D = 1; enum E {}",
        "t",
    )
    .unwrap();
    let fq: Vec<String> = p
        .stmts
        .iter()
        .filter_map(|s| match &s.kind {
            StmtKind::Function(f) => Some(f.fq_name.to_string()),
            StmtKind::Class(c) => Some(c.fq_name.to_string()),
            StmtKind::Const(c) => Some(c.consts[0].fq_name.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(fq, ["A\\B\\f", "A\\B\\C", "A\\B\\D", "A\\B\\E"]);
}
