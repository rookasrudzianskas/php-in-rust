//! `highlight_file`, `show_source`, `highlight_string` and
//! `php_strip_whitespace` (`Zend/zend_highlight.c`).

use php_runtime::ini::{IniDef, modifiable};
use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};
use php_syntax::{TokenKind, TokenizeFlags, tokenize};

use super::{open_script_source, path_arg};

/// `HL_*_COLOR` defaults (`Zend/zend_highlight.h`).
const HL_COMMENT_COLOR: &str = "#FF8000";
const HL_DEFAULT_COLOR: &str = "#0000BB";
const HL_HTML_COLOR: &str = "#000000";
const HL_KEYWORD_COLOR: &str = "#007700";
const HL_STRING_COLOR: &str = "#DD0000";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    Comment,
    Default,
    Html,
    Keyword,
    String,
}

struct Colors {
    comment: PhpStr,
    default: PhpStr,
    html: PhpStr,
    keyword: PhpStr,
    string: PhpStr,
}

impl Colors {
    /// `php_get_highlight_struct()`.
    fn from_ini(vm: &Vm) -> Colors {
        let get = |name: &str| vm.ini_get(name).unwrap_or_default();
        Colors {
            comment: get("highlight.comment"),
            default: get("highlight.default"),
            html: get("highlight.html"),
            keyword: get("highlight.keyword"),
            string: get("highlight.string"),
        }
    }

    fn css(&self, c: Color) -> &PhpStr {
        match c {
            Color::Comment => &self.comment,
            Color::Default => &self.default,
            Color::Html => &self.html,
            Color::Keyword => &self.keyword,
            Color::String => &self.string,
        }
    }
}

/// `zend_html_puts()`: HTML-escapes `<`, `>`, `&` and expands tabs.
fn html_puts(out: &mut Vec<u8>, text: &[u8]) {
    for &c in text {
        match c {
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'&' => out.extend_from_slice(b"&amp;"),
            b'\t' => out.extend_from_slice(b"    "),
            _ => out.push(c),
        }
    }
}

/// Whether `lex_scan()` stores a value (`zval`) for this token: those are
/// painted in the default colour instead of the keyword colour.
fn token_has_value(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LNumber
            | TokenKind::DNumber
            | TokenKind::String
            | TokenKind::NameFullyQualified
            | TokenKind::NameRelative
            | TokenKind::NameQualified
            | TokenKind::Variable
            | TokenKind::StringVarname
            | TokenKind::NumString
    )
}

/// `zend_highlight()`.
fn zend_highlight(source: &[u8], colors: &Colors) -> Vec<u8> {
    let mut out = Vec::new();
    let tokens = tokenize(source, TokenizeFlags::default()).unwrap_or_default();

    let mut last = Color::Html;
    out.extend_from_slice(b"<pre><code style=\"color: ");
    out.extend_from_slice(colors.css(last));
    out.extend_from_slice(b"\">");

    for tok in &tokens {
        let text = &source[tok.start as usize..(tok.start + tok.len) as usize];
        let next = match tok.kind {
            TokenKind::InlineHtml => Color::Html,
            TokenKind::Comment | TokenKind::DocComment => Color::Comment,
            TokenKind::OpenTag
            | TokenKind::OpenTagWithEcho
            | TokenKind::CloseTag
            | TokenKind::Line
            | TokenKind::File
            | TokenKind::Dir
            | TokenKind::TraitC
            | TokenKind::MethodC
            | TokenKind::FuncC
            | TokenKind::PropertyC
            | TokenKind::NsC
            | TokenKind::ClassC => Color::Default,
            TokenKind::DoubleQuote
            | TokenKind::EncapsedAndWhitespace
            | TokenKind::ConstantEncapsedString => Color::String,
            TokenKind::Whitespace => {
                // no colour needed
                html_puts(&mut out, text);
                continue;
            }
            k => {
                if token_has_value(k) {
                    Color::Default
                } else {
                    Color::Keyword
                }
            }
        };

        if last != next {
            if last != Color::Html {
                out.extend_from_slice(b"</span>");
            }
            last = next;
            if last != Color::Html {
                out.extend_from_slice(b"<span style=\"color: ");
                out.extend_from_slice(colors.css(last));
                out.extend_from_slice(b"\">");
            }
        }
        html_puts(&mut out, text);
    }

    if last != Color::Html {
        out.extend_from_slice(b"</span>");
    }
    out.extend_from_slice(b"</code></pre>");
    out
}

/// `zend_strip()`: source without comments and with whitespace runs folded.
fn zend_strip(source: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let tokens = tokenize(source, TokenizeFlags::default()).unwrap_or_default();
    let text = |t: &php_syntax::Token| &source[t.start as usize..(t.start + t.len) as usize];
    let mut prev_space = false;
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        i += 1;
        match tok.kind {
            TokenKind::Whitespace => {
                if !prev_space {
                    out.push(b' ');
                    prev_space = true;
                }
                continue;
            }
            TokenKind::Comment | TokenKind::DocComment => continue,
            TokenKind::EndHeredoc => {
                out.extend_from_slice(text(tok));
                // read the following character, either newline or ;
                match tokens.get(i) {
                    Some(next) if next.kind == TokenKind::Whitespace => i += 1,
                    Some(next) => {
                        out.extend_from_slice(text(next));
                        i += 1;
                    }
                    None => {}
                }
                out.push(b'\n');
                prev_space = true;
                continue;
            }
            _ => out.extend_from_slice(text(tok)),
        }
        prev_space = false;
    }
    out
}

/// `php_check_open_basedir()` (warning included): `Ok(true)` when the path
/// may be accessed.
fn check_open_basedir(vm: &mut Vm, path: &PhpStr) -> PResult<bool> {
    let ob = vm.ini_get("open_basedir").unwrap_or_default();
    if ob.is_empty() || path.is_empty() {
        return Ok(true);
    }
    let resolve = |p: &str| -> std::path::PathBuf {
        let pb = std::path::PathBuf::from(p);
        let abs = if pb.is_absolute() {
            pb
        } else {
            std::env::current_dir().unwrap_or_default().join(pb)
        };
        std::fs::canonicalize(&abs).unwrap_or(abs)
    };
    let target = resolve(&path.to_string_lossy());
    for dir in ob.to_string_lossy().split(':') {
        if dir.is_empty() {
            continue;
        }
        let allowed = resolve(dir);
        if target.starts_with(&allowed) {
            return Ok(true);
        }
    }
    vm.warning(format!(
        "open_basedir restriction in effect. File({path}) is not within the allowed path(s): ({ob})"
    ))?;
    Ok(false)
}

fn highlight_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_arg(vm, args, 0)?;
    let return_output = args.bool(1);

    if !check_open_basedir(vm, &filename)? {
        return Ok(Value::Bool(false));
    }
    let colors = Colors::from_ini(vm);
    let Some(source) = open_script_source(vm, &filename, true)? else {
        vm.warning(format!("Failed opening '{filename}' for highlighting"))?;
        return Ok(Value::Bool(false));
    };
    let out = zend_highlight(&source, &colors);
    if return_output {
        Ok(Value::String(PhpStr::from_vec(out)))
    } else {
        vm.echo(&out)?;
        Ok(Value::Bool(true))
    }
}

fn highlight_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let source = args.str(0).clone();
    let return_output = args.bool(1);
    let colors = Colors::from_ini(vm);
    let out = zend_highlight(&source, &colors);
    if return_output {
        Ok(Value::String(PhpStr::from_vec(out)))
    } else {
        vm.echo(&out)?;
        Ok(Value::Bool(true))
    }
}

fn php_strip_whitespace(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_arg(vm, args, 0)?;
    let Some(source) = open_script_source(vm, &filename, true)? else {
        return Ok(Value::empty_string());
    };
    Ok(Value::String(PhpStr::from_vec(zend_strip(&source))))
}

pub static HIGHLIGHT_FILE: NativeFunction = NativeFunction {
    sig: "highlight_file(string $filename, bool $return = false): string|bool",
    handler: highlight_file,
};
pub static SHOW_SOURCE: NativeFunction = NativeFunction {
    sig: "show_source(string $filename, bool $return = false): string|bool",
    handler: highlight_file,
};
pub static PHP_STRIP_WHITESPACE: NativeFunction = NativeFunction {
    sig: "php_strip_whitespace(string $filename): string",
    handler: php_strip_whitespace,
};
pub static HIGHLIGHT_STRING: NativeFunction = NativeFunction {
    sig: "highlight_string(string $string, bool $return = false): string|true",
    handler: highlight_string,
};

pub fn register(r: &mut Registry) {
    // `main/main.c` directives, owned by Core.
    for def in [
        IniDef::new("highlight.comment", Some(HL_COMMENT_COLOR), modifiable::ALL),
        IniDef::new("highlight.default", Some(HL_DEFAULT_COLOR), modifiable::ALL),
        IniDef::new("highlight.html", Some(HL_HTML_COLOR), modifiable::ALL),
        IniDef::new("highlight.keyword", Some(HL_KEYWORD_COLOR), modifiable::ALL),
        IniDef::new("highlight.string", Some(HL_STRING_COLOR), modifiable::ALL),
    ] {
        r.vm.ini.register(&def, "Core");
    }
    r.functions(&[
        &HIGHLIGHT_FILE,
        &SHOW_SOURCE,
        &PHP_STRIP_WHITESPACE,
        &HIGHLIGHT_STRING,
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn highlighted(t: &mut TestVm, src: &str) -> String {
        t.call("highlight_string", vec![Value::str(src), Value::Bool(true)])
            .unwrap()
            .as_str()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn span(color: &str, text: &str) -> String {
        format!("<span style=\"color: {color}\">{text}</span>")
    }

    #[test]
    fn simple_script_matches_highlight_file_phpt() {
        let mut t = TestVm::new();
        let got = highlighted(&mut t, "<?php echo \"test\"; ?>");
        let want = format!(
            "<pre><code style=\"color: #000000\">{}{}{}{}{}</code></pre>",
            span("#0000BB", "&lt;?php "),
            span("#007700", "echo "),
            span("#DD0000", "\"test\""),
            span("#007700", "; "),
            span("#0000BB", "?&gt;"),
        );
        assert_eq!(got, want);
    }

    #[test]
    fn unterminated_string_matches_phpt() {
        let mut t = TestVm::new();
        let got = highlighted(&mut t, "<?php echo \"test ?>");
        let want = format!(
            "<pre><code style=\"color: #000000\">{}{}{}</code></pre>",
            span("#0000BB", "&lt;?php "),
            span("#007700", "echo "),
            span("#DD0000", "\"test ?&gt;"),
        );
        assert_eq!(got, want);
    }

    #[test]
    fn class_example_matches_phpt() {
        let mut t = TestVm::new();
        let src = "\n<?php\nclass test {\n    public $var = 1;\n    private function foo() { echo \"foo\"; }\n    public function bar() { var_dump(test::foo()); }\n}\n?>";
        let got = highlighted(&mut t, src);
        let want = "<pre><code style=\"color: #000000\">\n<span style=\"color: #0000BB\">&lt;?php\n</span><span style=\"color: #007700\">class </span><span style=\"color: #0000BB\">test </span><span style=\"color: #007700\">{\n    public </span><span style=\"color: #0000BB\">$var </span><span style=\"color: #007700\">= </span><span style=\"color: #0000BB\">1</span><span style=\"color: #007700\">;\n    private function </span><span style=\"color: #0000BB\">foo</span><span style=\"color: #007700\">() { echo </span><span style=\"color: #DD0000\">\"foo\"</span><span style=\"color: #007700\">; }\n    public function </span><span style=\"color: #0000BB\">bar</span><span style=\"color: #007700\">() { </span><span style=\"color: #0000BB\">var_dump</span><span style=\"color: #007700\">(</span><span style=\"color: #0000BB\">test</span><span style=\"color: #007700\">::</span><span style=\"color: #0000BB\">foo</span><span style=\"color: #007700\">()); }\n}\n</span><span style=\"color: #0000BB\">?&gt;</span></code></pre>";
        assert_eq!(got, want);
    }

    #[test]
    fn colours_come_from_ini_and_output_can_be_echoed() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("highlight.keyword", "#112233");
        let r = t
            .call("highlight_string", vec![Value::str("<?php if;")])
            .unwrap();
        assert!(matches!(r, Value::Bool(true)));
        let out = t.output();
        assert!(
            out.contains("<span style=\"color: #112233\">if</span>")
                || out.contains("#112233\">if;"),
            "{out}"
        );
        assert!(out.starts_with("<pre><code style=\"color: #000000\">"));
    }

    #[test]
    fn comments_inline_html_and_tabs() {
        let mut t = TestVm::new();
        let got = highlighted(&mut t, "a<b>&\n<?php\t// c\n/** d */ $x;");
        assert!(
            got.starts_with("<pre><code style=\"color: #000000\">a&lt;b&gt;&amp;\n"),
            "{got}"
        );
        assert!(
            got.contains(&span("#FF8000", "// c\n/** d */"))
                || got.contains("<span style=\"color: #FF8000\">// c"),
            "{got}"
        );
        assert!(got.contains("&lt;?php    "), "{got}");
    }

    #[test]
    fn heredoc_and_magic_constants() {
        // bug67468: must not crash
        let mut t = TestVm::new();
        let got = highlighted(&mut t, "<?php __CLASS__;");
        assert_eq!(
            got,
            format!(
                "<pre><code style=\"color: #000000\">{}{}</code></pre>",
                span("#0000BB", "&lt;?php __CLASS__"),
                span("#007700", ";")
            )
        );
    }

    #[test]
    fn strip_whitespace_phpt_cases() {
        let strip = |s: &str| String::from_utf8(zend_strip(s.as_bytes())).unwrap();
        assert_eq!(strip("/* test comment */"), "/* test comment */"); // inline HTML
        assert_eq!(strip("<?php /* test comment */ ?>"), "<?php  ?>");
        let src = "<?php\n/* test class */\nclass test {\n\n    /* function foo () */\n\n    function foo () /* {{{ */\n    {\n\n\n        echo $var; //does not exist\n    }\n    /* }}} */\n\n}\n?>";
        assert_eq!(
            strip(src),
            "<?php\n class test { function foo () { echo $var; } } ?>"
        );
    }

    #[test]
    fn php_strip_whitespace_reads_files() {
        let mut t = TestVm::new();
        let path =
            std::env::temp_dir().join(format!("php_in_rust_strip_{}.php", std::process::id()));
        std::fs::write(&path, "<?php /* c */ echo   1;  // x\n").unwrap();
        let r = t
            .call(
                "php_strip_whitespace",
                vec![Value::str(path.to_str().unwrap())],
            )
            .unwrap();
        assert_eq!(r.as_str().unwrap(), "<?php  echo 1; ");
        let _ = std::fs::remove_file(&path);

        let r = t
            .call(
                "php_strip_whitespace",
                vec![Value::str("/no/such/file.php")],
            )
            .unwrap();
        assert_eq!(r.as_str().unwrap(), "");
        assert_eq!(
            t.output(),
            "\nWarning: php_strip_whitespace(): Failed to open stream: No such file or directory in /t.php on line 3\n"
        );
    }

    #[test]
    fn highlight_file_failure_and_success() {
        let mut t = TestVm::new();
        let r = t
            .call("highlight_file", vec![Value::str("/no/such/file.php")])
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: highlight_file(): Failed to open stream: No such file or directory in /t.php on line 3\n\nWarning: highlight_file(): Failed opening '/no/such/file.php' for highlighting in /t.php on line 3\n"
        );

        let path = std::env::temp_dir().join(format!("php_in_rust_hl_{}.php", std::process::id()));
        std::fs::write(&path, "<?php echo 1;").unwrap();
        let r = t
            .call(
                "show_source",
                vec![Value::str(path.to_str().unwrap()), Value::Bool(true)],
            )
            .unwrap();
        assert!(r.as_str().unwrap().starts_with(b"<pre><code"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_basedir_restricts_highlight_file() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("open_basedir", "/nonexistent-allowed");
        let r = t
            .call("highlight_file", vec![Value::str("/etc/hosts")])
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert!(t.output().contains("open_basedir restriction in effect. File(/etc/hosts) is not within the allowed path(s): (/nonexistent-allowed)"));
    }
}
