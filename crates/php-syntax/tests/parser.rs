//! Parser tests: precedence, constructs, semantic values, lines, doc
//! comments.

use php_syntax::ast::*;
use php_syntax::{ParseOptions, parse, parse_with};

fn prog(src: &str) -> Program {
    parse(src.as_bytes(), "t.php").unwrap_or_else(|e| panic!("{src}: {e}"))
}

fn first_expr(src: &str) -> Expr {
    let p = prog(&format!("<?php {src};"));
    match p.stmts.into_iter().next().map(|s| s.kind) {
        Some(StmtKind::Expr(e)) => e,
        other => panic!("not an expression statement: {other:?}"),
    }
}

/// Fully parenthesized rendering of operator structure.
fn sx(e: &Expr) -> String {
    use BinaryOp::*;
    match &e.kind {
        ExprKind::Int(i) => i.to_string(),
        ExprKind::Float(f) => format!("{f}"),
        ExprKind::String(s) => format!("'{s}'"),
        ExprKind::Var(VarName::Name(n)) => format!("${n}"),
        ExprKind::Const(n) => n.name.to_string(),
        ExprKind::Binary { op, left, right } => {
            let o = match op {
                Add => "+",
                Sub => "-",
                Mul => "*",
                Div => "/",
                Mod => "%",
                Pow => "**",
                Concat => ".",
                BitAnd => "&",
                BitOr => "|",
                BitXor => "^",
                ShiftLeft => "<<",
                ShiftRight => ">>",
                BooleanAnd => "&&",
                BooleanOr => "||",
                LogicalXor => "xor",
                Identical => "===",
                NotIdentical => "!==",
                Equal => "==",
                NotEqual => "!=",
                Less => "<",
                LessEqual => "<=",
                Greater => ">",
                GreaterEqual => ">=",
                Spaceship => "<=>",
                Coalesce => "??",
                Pipe => "|>",
            };
            format!("({} {o} {})", sx(left), sx(right))
        }
        ExprKind::Unary { op, expr } => {
            let o = match op {
                UnaryOp::Not => "!",
                UnaryOp::BitNot => "~",
                UnaryOp::Plus => "+",
                UnaryOp::Minus => "-",
            };
            format!("({o}{})", sx(expr))
        }
        ExprKind::Cast { expr, .. } => format!("(cast {})", sx(expr)),
        ExprKind::Assign { target, value } => format!("({} = {})", sx(target), sx(value)),
        ExprKind::AssignRef { target, value } => format!("({} =& {})", sx(target), sx(value)),
        ExprKind::AssignOp { target, value, .. } => format!("({} op= {})", sx(target), sx(value)),
        ExprKind::AssignCoalesce { target, value } => format!("({} ??= {})", sx(target), sx(value)),
        ExprKind::Ternary {
            cond,
            then: Some(t),
            else_,
            ..
        } => format!("({} ? {} : {})", sx(cond), sx(t), sx(else_)),
        ExprKind::Ternary {
            cond,
            then: None,
            else_,
            ..
        } => format!("({} ?: {})", sx(cond), sx(else_)),
        ExprKind::Instanceof { expr, .. } => format!("({} instanceof)", sx(expr)),
        ExprKind::Print(e) => format!("(print {})", sx(e)),
        ExprKind::Yield {
            key: Some(k),
            value: Some(v),
        } => format!("(yield {} => {})", sx(k), sx(v)),
        ExprKind::Yield {
            key: None,
            value: Some(v),
        } => format!("(yield {})", sx(v)),
        ExprKind::Yield { .. } => "(yield)".into(),
        ExprKind::YieldFrom(e) => format!("(yield from {})", sx(e)),
        ExprKind::Throw(e) => format!("(throw {})", sx(e)),
        ExprKind::Include { expr, .. } => format!("(include {})", sx(expr)),
        ExprKind::IncDec { op, target } => format!("({op:?} {})", sx(target)),
        ExprKind::Silence(e) => format!("(@{})", sx(e)),
        ExprKind::Call {
            callee: Callee::Name(n),
            args,
        } => {
            if n.name.eq_ignore_case(b"clone")
                && let Some(Arg::Positional(a)) = args.args.first()
            {
                return format!("(clone {})", sx(a));
            }
            format!("{}()", n.name)
        }
        ExprKind::Prop {
            object,
            name: MemberName::Ident(n),
            ..
        } => format!("{}->{n}", sx(object)),
        ExprKind::MethodCall {
            object,
            name: MemberName::Ident(n),
            ..
        } => format!("{}->{n}()", sx(object)),
        ExprKind::Dim {
            base,
            index: Some(i),
        } => format!("{}[{}]", sx(base), sx(i)),
        ExprKind::ArrowFn(f) => format!("(fn => {})", sx(&f.body)),
        _ => format!("<{:?}>", std::mem::discriminant(&e.kind)),
    }
}

fn p(src: &str) -> String {
    sx(&first_expr(src))
}

#[test]
fn arithmetic_precedence() {
    assert_eq!(p("1 + 2 * 3"), "(1 + (2 * 3))");
    assert_eq!(p("1 - 2 - 3"), "((1 - 2) - 3)");
    assert_eq!(p("2 ** 3 ** 2"), "(2 ** (3 ** 2))");
    assert_eq!(p("-2 ** 2"), "(-(2 ** 2))");
    assert_eq!(p("$a . $b + $c"), "($a . ($b + $c))");
    assert_eq!(p("$a << 1 + 2"), "($a << (1 + 2))");
    assert_eq!(p("$a . $b << 2"), "($a . ($b << 2))");
    assert_eq!(p("$a |> $b . $c"), "($a |> ($b . $c))");
    assert_eq!(p("$a == $b |> $c"), "($a == ($b |> $c))");
}

#[test]
fn logical_precedence() {
    assert_eq!(p("$a || $b && $c"), "($a || ($b && $c))");
    assert_eq!(p("$a or $b and $c"), "($a || ($b && $c))");
    assert_eq!(p("$a = $b or $c"), "(($a = $b) || $c)");
    assert_eq!(p("$a = $b || $c"), "($a = ($b || $c))");
    assert_eq!(p("$a xor $b or $c"), "(($a xor $b) || $c)");
    assert_eq!(p("$a & $b | $c ^ $d"), "(($a & $b) | ($c ^ $d))");
    assert_eq!(p("$a ?? $b ?? $c"), "($a ?? ($b ?? $c))");
    assert_eq!(p("$a ?? $b || $c"), "($a ?? ($b || $c))");
    assert_eq!(p("$a == 1 < 2"), "($a == (1 < 2))");
    assert_eq!(p("!$a instanceof B"), "(!($a instanceof))");
    assert_eq!(p("!$a + 1"), "((!$a) + 1)");
    assert_eq!(p("(int)$a + 1"), "((cast $a) + 1)");
    assert_eq!(p("@$a . $b"), "((@$a) . $b)");
}

#[test]
fn ternary_and_assignment() {
    assert_eq!(p("$a ? $b : $c ? $d : $e"), "(($a ? $b : $c) ? $d : $e)");
    assert_eq!(p("$a ? $b ? $c : $d : $e"), "($a ? ($b ? $c : $d) : $e)");
    assert_eq!(p("$a ?: $b ?: $c"), "(($a ?: $b) ?: $c)");
    assert_eq!(p("$x = $a ? $b : $c"), "($x = ($a ? $b : $c))");
    assert_eq!(p("$a = $b = $c"), "($a = ($b = $c))");
    assert_eq!(p("!$a = foo()"), "(!($a = foo()))");
    assert_eq!(p("$a + $b = 5"), "($a + ($b = 5))");
    assert_eq!(p("$a = &$b"), "($a =& $b)");
    assert_eq!(p("$a ??= $b ?? $c"), "($a ??= ($b ?? $c))");
    assert_eq!(p("$a += $b * 2"), "($a op= ($b * 2))");
    assert_eq!(p("-$a = 5"), "(-($a = 5))");
    assert_eq!(p("$a++ + ++$b"), "((PostInc $a) + (PreInc $b))");
}

#[test]
fn prefix_keyword_precedence() {
    assert_eq!(p("print $a and $b"), "((print $a) && $b)");
    assert_eq!(p("1 + print 2 + 3"), "(1 + (print (2 + 3)))");
    assert_eq!(p("yield $a or $b"), "((yield $a) || $b)");
    assert_eq!(p("yield $a => $b + 1"), "(yield $a => ($b + 1))");
    assert_eq!(p("$x = yield"), "($x = (yield))");
    assert_eq!(p("yield from $a . $b"), "(yield from ($a . $b))");
    assert_eq!(p("include $a or die()"), "(include ($a || exit()))");
    assert_eq!(p("throw $e ?? $f"), "(throw ($e ?? $f))");
    assert_eq!(p("clone $a->b"), "(clone $a->b)");
    assert_eq!(p("clone $a + 1"), "((clone $a) + 1)");
    assert_eq!(p("fn() => $a or $b"), "(fn => ($a || $b))");
}

#[test]
fn non_associative_operators_are_errors() {
    for src in [
        "1 == 2 == 3",
        "1 < 2 > 3",
        "$a <=> $b == $c",
        "$a === $b !== $c",
    ] {
        let err = parse(format!("<?php {src};").as_bytes(), "t").unwrap_err();
        assert!(
            err.message.starts_with("syntax error, unexpected token"),
            "{src}: {}",
            err.message
        );
    }
}

#[test]
fn dereferencing() {
    assert_eq!(p("$a->b->c()"), "$a->b->c()");
    assert_eq!(p("$a[1][2]"), "$a[1][2]");
    assert!(matches!(
        first_expr("new Foo()->bar()").kind,
        ExprKind::MethodCall { .. }
    ));
    assert!(matches!(
        first_expr("new Foo()['x']").kind,
        ExprKind::Dim { .. }
    ));
    assert!(parse(b"<?php new Foo->bar();", "t").is_err());
    assert!(matches!(
        first_expr("(function () {})()").kind,
        ExprKind::Call { .. }
    ));
    assert!(parse(b"<?php function () {}();", "t").is_err());
    assert!(matches!(
        first_expr("A::$b[0]()").kind,
        ExprKind::Call { .. }
    ));
    assert!(matches!(
        first_expr("A::$b()").kind,
        ExprKind::StaticCall { .. }
    ));
    assert!(matches!(
        first_expr("'strlen'('x')").kind,
        ExprKind::Call { .. }
    ));
    assert!(matches!(first_expr("FOO[0]").kind, ExprKind::Dim { .. }));
    assert!(matches!(
        first_expr("A::B::C").kind,
        ExprKind::ClassConst { .. }
    ));
    assert!(matches!(
        first_expr("$x::class").kind,
        ExprKind::ClassName(_)
    ));
    assert!(
        matches!(first_expr("foo(...)").kind, ExprKind::Call { ref args, .. } if args.is_callable_convert())
    );
    assert!(parse(b"<?php __CLASS__::foo();", "t").is_err());
    assert!(parse(b"<?php ($a) = 1;", "t").is_err());
    assert!(parse(b"<?php $a++ = 1;", "t").is_err());
    assert!(parse(b"<?php ++$a = 1;", "t").is_err());
}

#[test]
fn literals_and_strings() {
    let lit = |s: &str| first_expr(s).kind;
    assert_eq!(lit("0x1F"), ExprKind::Int(31));
    assert_eq!(lit("0b101"), ExprKind::Int(5));
    assert_eq!(lit("0o17"), ExprKind::Int(15));
    assert_eq!(lit("017"), ExprKind::Int(15));
    assert_eq!(lit("1_000_000"), ExprKind::Int(1_000_000));
    assert_eq!(lit("9223372036854775807"), ExprKind::Int(i64::MAX));
    assert_eq!(
        lit("9223372036854775808"),
        ExprKind::Float(9223372036854775808.0)
    );
    assert_eq!(
        lit("0xFFFFFFFFFFFFFFFF"),
        ExprKind::Float(18446744073709551615.0)
    );
    assert_eq!(lit("1.5e3"), ExprKind::Float(1500.0));
    assert_eq!(lit(".5"), ExprKind::Float(0.5));
    assert_eq!(lit("'a\\'b\\\\c\\n'"), ExprKind::String("a'b\\c\\n".into()));
    assert_eq!(
        lit("\"\\t\\x41\\101\\u{1F600}\\e\\$\""),
        ExprKind::String(b"\tAA\xF0\x9F\x98\x80\x1b$".to_vec().into())
    );
    assert_eq!(lit("\"\\u202e\""), ExprKind::String("\\u202e".into()));
    assert_eq!(lit("'a' . 'b' . 1"), ExprKind::String("ab1".into()));
    assert_eq!(
        lit("<<<EOT\n  a\n   b\n  EOT"),
        ExprKind::String("a\n b".into())
    );
    assert_eq!(
        lit("<<<'EOT'\n  a $x\n  EOT"),
        ExprKind::String("a $x".into())
    );
    assert_eq!(lit("<<<EOT\nEOT"), ExprKind::String("".into()));
    assert_eq!(lit("b'bin'"), ExprKind::String("bin".into()));
}

#[test]
fn interpolation() {
    let ExprKind::Interpolated(parts) =
        first_expr("\"a $b[0] $c->d {$e['f']} ${g} ${h['i']} $j[-1] $k[l] $m[-0]\"").kind
    else {
        panic!()
    };
    let kinds: Vec<String> = parts
        .iter()
        .map(|p| match p {
            StrPart::Literal(s) => format!("lit:{s}"),
            StrPart::Expr { expr, syntax } => {
                format!("{syntax:?}:{}", php_syntax::export_expr(expr))
            }
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "lit:a ",
            "Simple:$b[0]",
            "lit: ",
            "Simple:$c->d",
            "lit: ",
            "Simple:$e['f']",
            "lit: ",
            "DollarCurly:$g",
            "lit: ",
            "DollarCurly:$h['i']",
            "lit: ",
            "Simple:$j[-1]",
            "lit: ",
            "Simple:$k['l']",
            "lit: ",
            "Simple:$m['-0']",
        ]
    );
    let ExprKind::Interpolated(parts) = first_expr("<<<EOT\n  x $a\n  y\n  EOT").kind else {
        panic!()
    };
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[2], StrPart::Literal("\ny".into()));
}

#[test]
fn statements_and_alternative_syntax() {
    let src = "<?php
if ($a): echo 1; elseif ($b): echo 2; else: echo 3; endif;
while ($x): $x--; endwhile;
for ($i = 0, $j = 1; $i < 10; $i++): endfor;
foreach ($arr as $k => &$v): endforeach;
switch ($x): case 1: case 2; break; default: endswitch;
declare(ticks=1): enddeclare;
do { } while (0);
try { } catch (A|B $e) { } catch (C) { } finally { }
goto end; end:
static $s = 1, $t;
global $g, $$h;
unset($a[0], $b,);
echo 1, 2;
list($a, list(, $b), 'k' => [$c]) = $x;
[$a, [$b]] = $y;
foreach ($a as [$x, $y]) {}
?>html<?= $z ?>";
    let p = prog(src);
    let kinds: Vec<&str> = p
        .stmts
        .iter()
        .map(|s| match s.kind {
            StmtKind::If(_) => "if",
            StmtKind::While { .. } => "while",
            StmtKind::For(_) => "for",
            StmtKind::Foreach(_) => "foreach",
            StmtKind::Switch { .. } => "switch",
            StmtKind::Declare { .. } => "declare",
            StmtKind::DoWhile { .. } => "do",
            StmtKind::Try(_) => "try",
            StmtKind::Goto(_) => "goto",
            StmtKind::Label(_) => "label",
            StmtKind::Static(_) => "static",
            StmtKind::Global(_) => "global",
            StmtKind::Unset(_) => "unset",
            StmtKind::Echo(_) => "echo",
            StmtKind::Expr(_) => "expr",
            StmtKind::InlineHtml(_) => "html",
            StmtKind::Nop => "nop",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "if", "while", "for", "foreach", "switch", "declare", "do", "try", "goto", "label",
            "static", "global", "unset", "echo", "expr", "expr", "foreach", "nop", "html", "echo"
        ]
    );
    let StmtKind::Switch { cases, .. } = &p.stmts[4].kind else {
        panic!()
    };
    assert_eq!(cases.len(), 3);
    assert!(cases[1].semicolon);
    let StmtKind::Try(t) = &p.stmts[7].kind else {
        panic!()
    };
    assert_eq!(t.catches[0].types.len(), 2);
    assert_eq!(t.catches[1].var, None);
}

#[test]
fn declarations() {
    let src = r#"<?php
namespace App\Models;
use Foo\{Bar, function baz, const QUX};
#[Attr(1), Other]
abstract class A extends B implements C, D {
    use T1, T2 { T1::m insteadof T2; m as protected n; }
    final public const int X = 1, Y = 2;
    public private(set) ?int $p = null;
    public string $hooked { get => $this->p; set(string $v) { $this->p = $v; } }
    var $old;
    public function __construct(private readonly int $x = 0, protected(set) string ...$rest) {}
    abstract protected static function &f(int|string $a, (A&B)|null $b, A&B $c, ?self $d): static;
}
interface I extends J, K { public function m(); }
trait T { public function t() { return __TRAIT__; } }
enum Suit: string implements HasLabel { case Hearts = 'H'; case Spades = 'S'; const X = self::Hearts; }
final readonly class R {}
function &gen() { yield 1; }
$f = static fn&(int $x): int => $x;
$g = function () use ($a, &$b) { return __FUNCTION__ . __PROPERTY__; };
$o = new readonly class(1) extends A {};
$m = match ($x) { 1, 2 => 'a', default => 'b', };
const TOP = 1;
"#;
    let p = prog(src);
    let StmtKind::Class(a) = &p.stmts[2].kind else {
        panic!("{:?}", p.stmts[2].kind)
    };
    assert_eq!(a.fq_name, "App\\Models\\A");
    assert!(a.modifiers.is_abstract);
    assert_eq!(a.attributes[0].len(), 2);
    assert_eq!(a.extends.as_ref().unwrap().resolved, "App\\Models\\B");
    assert_eq!(a.members.len(), 7);
    let ClassMemberKind::Method(ctor) = &a.members[5].kind else {
        panic!()
    };
    assert!(
        ctor.common.params[0]
            .modifiers
            .contains(Modifiers::PRIVATE | Modifiers::READONLY)
    );
    assert!(ctor.common.params[1].variadic);
    let ClassMemberKind::Method(f) = &a.members[6].kind else {
        panic!()
    };
    assert!(f.body.is_none());
    assert!(f.common.by_ref);
    assert!(matches!(f.common.return_type, Some(TypeHint::Static)));
    assert!(
        matches!(&f.common.params[1].ty, Some(TypeHint::Union(u)) if matches!(u[0], TypeHint::Intersection(_)))
    );
    let ClassMemberKind::Property(h) = &a.members[3].kind else {
        panic!()
    };
    assert_eq!(h.props[0].hooks.as_ref().unwrap().len(), 2);
    let ClassMemberKind::Property(ps) = &a.members[2].kind else {
        panic!()
    };
    assert!(ps.modifiers.contains(Modifiers::PRIVATE_SET));
    let StmtKind::Class(e) = &p.stmts[5].kind else {
        panic!()
    };
    assert_eq!(e.kind, ClassKind::Enum);
    assert!(matches!(
        e.enum_backing_type,
        Some(TypeHint::Builtin {
            ty: BuiltinType::String,
            ..
        })
    ));
    let StmtKind::Function(g) = &p.stmts[7].kind else {
        panic!()
    };
    assert!(g.common.is_generator && g.common.by_ref);
    assert_eq!(g.fq_name, "App\\Models\\gen");
    let StmtKind::Const(c) = &p.stmts[12].kind else {
        panic!()
    };
    assert_eq!(c.consts[0].fq_name, "App\\Models\\TOP");
}

#[test]
fn modifier_errors_are_compile_errors() {
    let cases = [
        (
            "<?php class A { public public $x; }",
            "Multiple access type modifiers are not allowed",
        ),
        (
            "<?php class A { abstract final function f(); }",
            "Cannot use the final modifier on an abstract method",
        ),
        (
            "<?php class A { static const X = 1; }",
            "Cannot use the static modifier on a class constant",
        ),
        (
            "<?php class A { readonly function f() {} }",
            "Cannot use the readonly modifier on a method",
        ),
        (
            "<?php class A { function __construct(static $x) {} }",
            "Cannot use the static modifier on a parameter",
        ),
        (
            "<?php final final class A {}",
            "Multiple final modifiers are not allowed",
        ),
        (
            "<?php abstract final class A {}",
            "Cannot use the final modifier on an abstract class",
        ),
        (
            "<?php new final class {};",
            "Cannot use the final modifier on an anonymous class",
        ),
        (
            "<?php class A { public $x { public get; } }",
            "Cannot use the public modifier on a property hook",
        ),
        (
            "<?php function f() { __halt_compiler(); }",
            "__HALT_COMPILER() can only be used from the outermost scope",
        ),
    ];
    for (src, msg) in cases {
        let e = parse(src.as_bytes(), "t").unwrap_err();
        assert_eq!(e.kind, php_syntax::ParseErrorKind::Compile, "{src}");
        assert_eq!(e.message, msg, "{src}");
    }
}

#[test]
fn lines() {
    let src = "<?php\n$a =\n  1 +\n  2;\nfoo(\n  $b\n);\n$x\n  ->y()\n  ->z();\nfunction f() {\n}\necho __LINE__;\n";
    let p = prog(src);
    assert_eq!(p.stmts[0].line, 2);
    assert_eq!(p.stmts[1].line, 5);
    let StmtKind::Expr(e) = &p.stmts[2].kind else {
        panic!()
    };
    assert_eq!(e.line, 8, "method chains report the line of their start");
    let StmtKind::Function(f) = &p.stmts[3].kind else {
        panic!()
    };
    assert_eq!((f.common.start_line, f.common.end_line), (11, 12));
    let StmtKind::Echo(es) = &p.stmts[4].kind else {
        panic!()
    };
    assert_eq!(es[0].line, 13);
    // `?>\n` defers the line increment, heredoc bodies count their lines.
    let p = prog("<?php $a = <<<EOT\nx\ny\nEOT;\n$b;?>\n<?php $c;");
    assert_eq!(
        p.stmts.iter().map(|s| s.line).collect::<Vec<_>>(),
        [1, 5, 5, 6]
    );
}

#[test]
fn doc_comments() {
    let src = "<?php\n/** fdoc */\nfunction f() {}\n/** cdoc */\n#[A]\nclass C {\n    /** pdoc */\n    public $p;\n    /** mdoc */\n    public function m(/** argdoc */ $a) {}\n    /** kdoc */\n    const K = 1;\n}\n";
    let p = prog(src);
    let StmtKind::Function(f) = &p.stmts[0].kind else {
        panic!()
    };
    assert_eq!(f.common.doc_comment.as_ref().unwrap(), "/** fdoc */");
    let StmtKind::Class(c) = &p.stmts[1].kind else {
        panic!()
    };
    assert_eq!(c.doc_comment.as_ref().unwrap(), "/** cdoc */");
    let ClassMemberKind::Property(pg) = &c.members[0].kind else {
        panic!()
    };
    assert_eq!(pg.props[0].doc_comment.as_ref().unwrap(), "/** pdoc */");
    let ClassMemberKind::Method(m) = &c.members[1].kind else {
        panic!()
    };
    assert_eq!(m.common.doc_comment.as_ref().unwrap(), "/** mdoc */");
    assert_eq!(
        m.common.params[0].doc_comment.as_ref().unwrap(),
        "/** argdoc */"
    );
    let ClassMemberKind::Const(k) = &c.members[2].kind else {
        panic!()
    };
    assert_eq!(k.consts[0].doc_comment.as_ref().unwrap(), "/** kdoc */");
    // A closing brace resets the pending doc comment.
    let p = prog("<?php /** x */ { } function g() {}");
    let StmtKind::Function(g) = &p.stmts[1].kind else {
        panic!()
    };
    assert!(g.common.doc_comment.is_none());
}

#[test]
fn halt_compiler_offset() {
    let src = b"<?php echo 1;\n__halt_compiler(); binary \x00\xff";
    let p = parse(src, "t").unwrap();
    let off = p.halt_offset.unwrap();
    assert_eq!(&src[off..], b" binary \x00\xff");
}

#[test]
fn deprecations_are_reported() {
    let p = prog("<?php $a = (integer) $b; $c = \"\\400\";");
    let msgs: Vec<_> = p.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(
        msgs,
        [
            "Non-canonical cast (integer) is deprecated, use the (int) cast instead",
            "Octal escape sequence overflow \\400 is greater than \\377"
        ]
    );
    let e = parse(b"<?php (real) $x;", "t").unwrap_err();
    assert_eq!(
        e.message,
        "The (real) cast has been removed, use (float) instead"
    );
}

#[test]
fn eval_mode_and_short_tags() {
    let opts = ParseOptions {
        start_in_scripting: true,
        ..Default::default()
    };
    let p = parse_with(b"return 1 + 2;", "eval", &opts).unwrap();
    assert!(matches!(p.stmts[0].kind, StmtKind::Return(Some(_))));
    let opts = ParseOptions {
        short_open_tag: true,
        ..Default::default()
    };
    let p = parse_with(b"<? echo 1;", "t", &opts).unwrap();
    assert!(matches!(p.stmts[0].kind, StmtKind::Echo(_)));
    let p = parse(b"<? echo 1;", "t").unwrap();
    assert!(matches!(p.stmts[0].kind, StmtKind::InlineHtml(_)));
    let opts = ParseOptions {
        skip_shebang: true,
        ..Default::default()
    };
    let p = parse_with(b"#!/usr/bin/env php\n<?php echo __LINE__;", "t", &opts).unwrap();
    let StmtKind::Echo(es) = &p.stmts[0].kind else {
        panic!()
    };
    assert_eq!(es[0].line, 2);
}

#[test]
fn deep_nesting_does_not_overflow() {
    // Well within bison's stack: parsed (on a large stack if needed).
    let depth = 2000;
    let src = format!("<?php $a = {}1{};", "(".repeat(depth), ")".repeat(depth));
    let src2 = src.clone();
    // Run on a small thread stack to prove the parser does not rely on it.
    let r = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || parse(src2.as_bytes(), "t").map(|p| p.stmts.len()))
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(r.unwrap(), 1);
    // Beyond bison's 10000-entry stack: php-src reports "memory exhausted".
    let src = format!("<?php $a = {}1{};", "[".repeat(12000), "]".repeat(12000));
    let e = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || parse(src.as_bytes(), "t").unwrap_err())
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(e.message, "memory exhausted");
}
