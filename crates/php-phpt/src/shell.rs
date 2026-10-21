//! Small PHP-compatible string helpers: `escapeshellarg`, `addslashes`, `trim`.

/// The characters PHP's `trim()` strips by default.
const PHP_TRIM_CHARS: [char; 6] = [' ', '\t', '\n', '\r', '\0', '\x0B'];

/// `trim()` for text (default character set).
pub fn trim_php(s: &str) -> &str {
    s.trim_matches(&PHP_TRIM_CHARS[..])
}

/// `rtrim()` for text (default character set).
pub fn rtrim_php(s: &str) -> &str {
    s.trim_end_matches(&PHP_TRIM_CHARS[..])
}

/// `ltrim()` for text (default character set).
pub fn ltrim_php(s: &str) -> &str {
    s.trim_start_matches(&PHP_TRIM_CHARS[..])
}

/// PHP's `empty()` applied to a string: `""` and `"0"` are empty.
pub fn php_empty(s: &str) -> bool {
    s.is_empty() || s == "0"
}

/// POSIX `escapeshellarg`: wrap in single quotes, escaping embedded quotes.
pub fn escape_arg(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for c in arg.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// `escaped_shell_string_from()`: escape every argument and join with spaces.
pub fn escape_args<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|a| escape_arg(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// PHP's `addslashes`: backslash-escape `'`, `"`, `\` and NUL.
pub fn addslashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\'' | '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_arg_quotes_like_php() {
        assert_eq!(escape_arg("abc"), "'abc'");
        assert_eq!(escape_arg("it's"), "'it'\\''s'");
        assert_eq!(escape_arg(""), "''");
        assert_eq!(escape_args(&["-n", "a b"]), "'-n' 'a b'");
    }

    #[test]
    fn addslashes_matches_php() {
        assert_eq!(addslashes(r#"a'b"c\d"#), r#"a\'b\"c\\d"#);
        assert_eq!(addslashes("nul\0"), "nul\\0");
    }

    #[test]
    fn trim_helpers_use_php_character_set() {
        assert_eq!(trim_php(" \t\r\nabc\0\x0B "), "abc");
        assert_eq!(rtrim_php("abc \n"), "abc");
        assert_eq!(ltrim_php("\n abc "), "abc ");
        assert!(php_empty("0"));
        assert!(php_empty(""));
        assert!(!php_empty("00"));
    }
}
