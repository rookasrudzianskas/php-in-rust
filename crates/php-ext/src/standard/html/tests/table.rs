//! get_html_translation_table() phpt ports.

use php_runtime::{ArrayKey, PhpArray, Value};

use super::super::*;
use super::*;

fn table(p: &mut Php, which: i64, flags: i64, charset: &str) -> Value {
    p.call(
        "get_html_translation_table",
        vec![i(which), i(flags), s(charset)],
    )
}

/// `uksort()`/`uasort()` with a comparison of the first bytes (of keys or values).
fn sort_first_byte(v: &Value, by_key: bool) -> Value {
    let Value::Array(a) = v else {
        panic!("expected an array");
    };
    let mut items: Vec<(ArrayKey, Value)> = a.iter().map(|(k, v)| (k, v.clone())).collect();
    let first = |(k, v): &(ArrayKey, Value)| -> u8 {
        let b = if by_key {
            match k {
                ArrayKey::Int(n) => n.to_string().into_bytes(),
                ArrayKey::Str(s) => s.to_vec(),
            }
        } else {
            bytes(v)
        };
        b.first().copied().unwrap_or(0)
    };
    items.sort_by_key(|item| first(item));
    let mut out = PhpArray::new();
    for (k, v) in items {
        out.insert(k, v);
    }
    Value::array(out)
}

#[test]
fn basic1_basic2_basic3() {
    let mut p = Php::new();
    p.echo("*** Testing get_html_translation_table() : basic functionality ***\n");
    p.echo("-- with table = HTML_ENTITIES --\n");
    let tt = table(&mut p, PHP_HTML_ENTITIES, ENT_COMPAT, "UTF-8");
    p.var_dump(&asort(&tt));
    p.echo("-- with table = HTML_SPECIALCHARS --\n");
    let tt = table(&mut p, PHP_HTML_SPECIALCHARS, ENT_COMPAT, "UTF-8");
    p.var_dump(&asort(&tt));
    p.echo("Done\n");
    p.check("get_html_translation_table_basic1.phpt");

    for (name, which, label) in [
        (
            "get_html_translation_table_basic2.phpt",
            PHP_HTML_ENTITIES,
            "HTML_ENTITIES",
        ),
        (
            "get_html_translation_table_basic3.phpt",
            PHP_HTML_SPECIALCHARS,
            "HTML_SPECIALCHARS",
        ),
    ] {
        let mut p = Php::new();
        p.echo("*** Testing get_html_translation_table() : basic functionality ***\n");
        for (flags, style) in [
            (ENT_COMPAT, "ENT_COMPAT"),
            (ENT_QUOTES, "ENT_QUOTES"),
            (ENT_NOQUOTES, "ENT_NOQUOTES"),
        ] {
            p.echo(format!(
                "-- with table = {label} & quote_style = {style} --\n"
            ));
            let tt = table(&mut p, which, flags, "UTF-8");
            p.var_dump(&asort(&tt));
        }
        p.echo("Done\n");
        p.check(name);
    }
}

#[test]
fn basic4_windows1252() {
    let mut p = Php::new();
    p.echo("*** Testing get_html_translation_table() : basic functionality/Windows-1252 ***\n");
    p.echo("-- with table = HTML_ENTITIES --\n");
    let tt = table(&mut p, PHP_HTML_ENTITIES, ENT_COMPAT, "WINDOWS-1252");
    p.var_dump(&asort(&tt));
    p.echo("-- with table = HTML_SPECIALCHARS --\n");
    let tt = table(&mut p, PHP_HTML_SPECIALCHARS, ENT_COMPAT, "WINDOWS-1252");
    p.var_dump(&asort(&tt));
    p.echo("Done\n");
    p.check("get_html_translation_table_basic4.phpt");
}

/// basic5 (HTML 5), basic7 (XHTML 1.0), basic8 (XML 1.0) and basic9 (HTML 5/SJIS) share a
/// structure; basic6 (HTML 5/Windows-1251) sorts with `uksort()` instead.
#[test]
fn basic5_to_basic9_doctypes() {
    for (name, title, doctype, charset, first_flags) in [
        (
            "get_html_translation_table_basic5.phpt",
            "HTML 5",
            ENT_HTML5,
            "UTF-8",
            ENT_COMPAT,
        ),
        (
            "get_html_translation_table_basic6.phpt",
            "HTML 5/Windows-1251",
            ENT_HTML5,
            "Windows-1251",
            ENT_COMPAT,
        ),
        (
            "get_html_translation_table_basic7.phpt",
            "XHTML 1.0",
            ENT_XHTML,
            "UTF-8",
            ENT_QUOTES,
        ),
        (
            "get_html_translation_table_basic8.phpt",
            "XML 1.0",
            ENT_XML1,
            "UTF-8",
            ENT_QUOTES,
        ),
        (
            "get_html_translation_table_basic9.phpt",
            "HTML5/SJIS",
            ENT_HTML5,
            "SJIS",
            ENT_QUOTES,
        ),
    ] {
        let uksort = name.ends_with("basic6.phpt");
        let sort = |tt: &Value| {
            if uksort {
                sort_first_byte(tt, true)
            } else {
                asort(tt)
            }
        };
        let style = |f: i64| match f {
            ENT_COMPAT => "ENT_COMPAT",
            ENT_QUOTES => "ENT_QUOTES",
            _ => "ENT_NOQUOTES",
        };
        let mut p = Php::new();
        if uksort {
            p.echo(format!(
                "*** Testing get_html_translation_table() : basic functionality - {title} ***\n"
            ));
        } else {
            p.echo(format!(
                "*** Testing get_html_translation_table() : basic functionality/{title} ***\n"
            ));
        }
        if charset == "SJIS" {
            p.echo("*** Only basic entities supported! ***\n");
        }
        let others = if first_flags == ENT_COMPAT {
            [ENT_QUOTES, ENT_NOQUOTES]
        } else {
            [ENT_COMPAT, ENT_NOQUOTES]
        };
        p.echo(format!(
            "-- with table = HTML_ENTITIES, {} --\n",
            style(first_flags)
        ));
        let tt = table(&mut p, PHP_HTML_ENTITIES, first_flags | doctype, charset);
        p.var_dump(&i(count(&tt) as i64));
        p.print_r(&sort(&tt));
        for f in others {
            p.echo(format!("-- with table = HTML_ENTITIES, {} --\n", style(f)));
            let tt = table(&mut p, PHP_HTML_ENTITIES, f | doctype, charset);
            p.var_dump(&i(count(&tt) as i64));
        }
        p.echo("-- with table = HTML_SPECIALCHARS, ENT_COMPAT --\n");
        let tt = table(&mut p, PHP_HTML_SPECIALCHARS, ENT_COMPAT, charset);
        p.var_dump(&i(count(&tt) as i64));
        p.print_r(&sort(&tt));
        p.echo("-- with table = HTML_SPECIALCHARS, ENT_QUOTES --\n");
        let tt = table(&mut p, PHP_HTML_SPECIALCHARS, ENT_QUOTES | doctype, charset);
        p.var_dump(&sort(&tt));
        p.echo("-- with table = HTML_SPECIALCHARS, ENT_NOQUOTES --\n");
        let tt = table(
            &mut p,
            PHP_HTML_SPECIALCHARS,
            ENT_NOQUOTES | doctype,
            charset,
        );
        // basic6 uses uasort() here, with all values starting with '&'
        p.var_dump(&if uksort {
            sort_first_byte(&tt, false)
        } else {
            asort(&tt)
        });
        p.echo("Done\n");
        p.check(name);
    }
}

#[test]
fn basic10_iso8859_1() {
    let mut p = Php::new();
    let tt = table(&mut p, PHP_HTML_ENTITIES, ENT_COMPAT, "ISO-8859-1");
    p.var_dump(&i(count(&tt) as i64));
    p.print_r(&sort_first_byte(&tt, true));
    p.echo("Done\n");
    p.check("get_html_translation_table_basic10.phpt");
}

#[test]
fn bug73817_all_entities_end_with_semicolon() {
    let mut p = Php::new();
    let tt = p.call(
        "get_html_translation_table",
        vec![i(PHP_HTML_ENTITIES), i(ENT_QUOTES | ENT_HTML5)],
    );
    for (_, v) in entries(&tt) {
        if !v.ends_with(b";") {
            p.dump(v);
        }
    }
    p.echo("===DONE===\n");
    p.check("bug73817.phpt");
}

#[test]
fn bug54322_float_table_argument() {
    let mut p = Php::new();
    let r = p.t.call(
        "get_html_translation_table",
        vec![Value::Float(f64::NAN), i(0), s("UTF-8")],
    );
    let (class, msg) = crate::testing::thrown(r);
    p.echo(format!("{class}: {msg}\n"));
    p.check("bug54322.phpt");
}
