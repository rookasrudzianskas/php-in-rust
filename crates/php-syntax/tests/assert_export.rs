//! Data-driven check of the AST exporter against php-src's assertion tests:
//! for every `assert(<expr>)` in `Zend/tests/assert/*.phpt` (and a few other
//! directories), the expected output must contain the exported
//! `assert(<expr>)` text.

use std::path::Path;

use php_syntax::ast::*;
use php_syntax::{ParseOptions, export_expr, parse_with};

fn sections(data: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in data.split_inclusive('\n') {
        let t = line.trim_end();
        if t.len() > 4
            && t.starts_with("--")
            && t.ends_with("--")
            && t[2..t.len() - 2]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_')
        {
            out.push((t[2..t.len() - 2].to_string(), String::new()));
        } else if let Some(l) = out.last_mut() {
            l.1.push_str(line);
        }
    }
    out
}

/// Collect the single argument of every `assert(...)` call.
fn collect_asserts<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    match &e.kind {
        ExprKind::Call { callee, args } => {
            if let Callee::Name(n) = callee
                && n.name.eq_ignore_case(b"assert")
                && args.args.len() == 1
                && let Arg::Positional(a) = &args.args[0]
            {
                out.push(a);
                return;
            }
            for a in &args.args {
                if let Arg::Positional(a) | Arg::Named { value: a, .. } = a {
                    collect_asserts(a, out);
                }
            }
        }
        ExprKind::MethodCall { args, .. }
        | ExprKind::StaticCall { args, .. }
        | ExprKind::New { args, .. } => {
            for a in &args.args {
                if let Arg::Positional(a) | Arg::Named { value: a, .. } = a {
                    collect_asserts(a, out);
                }
            }
        }
        ExprKind::Assign { value, .. } => collect_asserts(value, out),
        ExprKind::Closure(c) => walk_stmts(&c.body, out),
        _ => {}
    }
}

fn walk_stmts<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Expr>) {
    for s in stmts {
        match &s.kind {
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => collect_asserts(e, out),
            StmtKind::Echo(es) => es.iter().for_each(|e| collect_asserts(e, out)),
            StmtKind::Try(t) => {
                walk_stmts(&t.body, out);
                t.catches.iter().for_each(|c| walk_stmts(&c.body, out));
            }
            StmtKind::Block(b)
            | StmtKind::While { body: b, .. }
            | StmtKind::Namespace { body: Some(b), .. } => walk_stmts(b, out),
            StmtKind::Foreach(f) => walk_stmts(&f.body, out),
            StmtKind::If(i) => {
                i.branches.iter().for_each(|b| walk_stmts(&b.body, out));
                if let Some(e) = &i.else_branch {
                    walk_stmts(e, out);
                }
            }
            StmtKind::Function(f) => walk_stmts(&f.body, out),
            StmtKind::Class(c) => {
                for m in &c.members {
                    if let ClassMemberKind::Method(m) = &m.kind
                        && let Some(b) = &m.body
                    {
                        walk_stmts(b, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn phpt_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            phpt_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "phpt") {
            out.push(p);
        }
    }
}

#[test]
fn assert_messages_match_php_src() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../php-src-ref"));
    let mut files = Vec::new();
    phpt_files(&root.join("Zend/tests"), &mut files);
    phpt_files(&root.join("ext/standard/tests/assert"), &mut files);
    if files.is_empty() {
        eprintln!("php-src-ref not available, skipping");
        return;
    }
    let mut checked = 0;
    let mut failures = Vec::new();
    let mut skipped = Vec::new();
    for path in files {
        let data = std::fs::read_to_string(&path).unwrap_or_default();
        if !data.contains("assert(") {
            continue;
        }
        let secs = sections(&data);
        let Some((_, file)) = secs.iter().find(|(n, _)| n == "FILE") else {
            continue;
        };
        let expected: String = secs
            .iter()
            .filter(|(n, _)| n.starts_with("EXPECT"))
            .map(|(_, c)| c.as_str())
            .collect();
        let opts = ParseOptions {
            no_resolve: true,
            ..Default::default()
        };
        let Ok(prog) = parse_with(file.as_bytes(), "t.php", &opts) else {
            continue;
        };
        let mut asserts = Vec::new();
        walk_stmts(&prog.stmts, &mut asserts);
        for a in asserts {
            let msg = format!("assert({})", export_expr(a));
            // Only check assertions whose message appears to be printed.
            let head: String = msg.chars().take(12).collect();
            if !expected.contains(&head) {
                skipped.push(msg.clone());
                continue;
            }
            checked += 1;
            if !expected.contains(&msg) {
                failures.push(format!("{}:\n{msg}", Path::new(&path).display()));
            }
        }
    }
    if std::env::var_os("SHOW_SKIPPED").is_some() {
        eprintln!(
            "checked {checked}, skipped {}:\n{}",
            skipped.len(),
            skipped.join("\n")
        );
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(checked > 10, "only {checked} assertions checked");
}
