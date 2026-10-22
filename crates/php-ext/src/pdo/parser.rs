//! `pdo_sql_parser.re`: placeholder scanning and query rewriting for
//! emulated prepares (`?` and `:name` placeholders, quoting of bound
//! values, `?`→`:pdoN` and `:name`→`?` rewriting).
//!
//! The re2c scanners are reproduced as longest-match tokenizers: the query
//! ends at its end or at the first NUL byte; an unterminated block comment or
//! a trailing `--` comment ends the scan (re2c's `YYFILL` returns EOI).

use php_runtime::{PResult, PhpStr, Value, Vm};

use super::consts::{PARAM_BOOL, PARAM_INT, PARAM_LOB, PARAM_NULL};
use super::dbh::Dbh;
use super::error::raise_impl_error;
use super::stmt::{PLACEHOLDER_NAMED, PLACEHOLDER_NONE, PLACEHOLDER_POSITIONAL, ParamKey, Stmt};

/// Token kinds (`PDO_PARSER_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Text,
    Bind,
    BindPos,
    EscapedQuestion,
    CustomQuote,
    Eoi,
}

/// A scanned token ending at `end` (exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub end: usize,
}

/// The input up to the first NUL (C string semantics of the scanners).
fn effective(s: &[u8]) -> &[u8] {
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
}

/// `"..."`-style literal: `q ( qq | other )* q`; `allow_inner` lets the
/// body contain the quote itself (the SQLite scanner's `ANYNOEOF*`).
fn quoted_len(s: &[u8], i: usize, q: u8, allow_inner: bool) -> usize {
    if s.get(i) != Some(&q) {
        return 0;
    }
    let mut best = 0;
    let mut j = i + 1;
    while j < s.len() {
        if s[j] == q {
            best = j + 1 - i;
            if allow_inner || s.get(j + 1) == Some(&q) {
                j += if allow_inner { 1 } else { 2 };
                continue;
            }
            break;
        }
        j += 1;
    }
    best
}

/// `"[" ANYNOEOF* "]"` (longest: up to the last `]`).
fn bracket_len(s: &[u8], i: usize) -> usize {
    if s.get(i) != Some(&b'[') {
        return 0;
    }
    s[i + 1..]
        .iter()
        .rposition(|&c| c == b']')
        .map_or(0, |p| p + 2)
}

fn run_len(s: &[u8], i: usize, c: u8) -> usize {
    s[i..].iter().take_while(|&&x| x == c).count()
}

/// `MULTICHAR = ([:]{2,}|[?]{2,})`.
fn multichar_len(s: &[u8], i: usize) -> usize {
    match s[i] {
        b':' | b'?' => {
            let n = run_len(s, i, s[i]);
            if n >= 2 { n } else { 0 }
        }
        _ => 0,
    }
}

/// `BINDCHR = [:][a-zA-Z0-9_]+`.
fn bind_len(s: &[u8], i: usize) -> usize {
    if s[i] != b':' {
        return 0;
    }
    let n = s[i + 1..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
        .count();
    if n > 0 { n + 1 } else { 0 }
}

/// `COMMENTS`; `Err(())` when the comment runs into the end of the input.
fn comment_len(s: &[u8], i: usize) -> Result<usize, ()> {
    if s[i..].starts_with(b"/*") {
        return match s[i + 2..].windows(2).position(|w| w == b"*/") {
            Some(p) => Ok(p + 4),
            None => Err(()),
        };
    }
    if s[i..].starts_with(b"--") {
        return match s[i + 2..].iter().position(|&c| c == b'\n') {
            Some(p) => Ok(p + 2),
            None => Err(()),
        };
    }
    Ok(0)
}

fn scan(s: &[u8], i: usize, specials: &[u8], sqlite: bool) -> Token {
    let s = effective(s);
    if i >= s.len() {
        return Token {
            kind: TokenKind::Eoi,
            end: i,
        };
    }
    // Rules in re2c priority order; the longest match wins.
    let mut best: (usize, TokenKind) = (0, TokenKind::Text);
    let mut consider = |len: usize, kind: TokenKind| {
        if len > best.0 {
            best = (len, kind);
        }
    };
    consider(quoted_len(s, i, b'"', sqlite), TokenKind::Text);
    consider(quoted_len(s, i, b'\'', sqlite), TokenKind::Text);
    if sqlite {
        consider(quoted_len(s, i, b'`', true), TokenKind::Text);
        consider(bracket_len(s, i), TokenKind::Text);
    }
    consider(multichar_len(s, i), TokenKind::Text);
    consider(bind_len(s, i), TokenKind::Bind);
    consider((s[i] == b'?') as usize, TokenKind::BindPos);
    consider(specials.contains(&s[i]) as usize, TokenKind::Text);
    match comment_len(s, i) {
        Ok(n) => consider(n, TokenKind::Text),
        Err(()) => {
            return Token {
                kind: TokenKind::Eoi,
                end: s.len(),
            };
        }
    }
    let plain = s[i..].iter().take_while(|c| !specials.contains(c)).count();
    consider(plain, TokenKind::Text);
    Token {
        kind: best.1,
        end: i + best.0.max(1),
    }
}

/// The default scanner (`default_scanner`).
pub fn default_scanner(s: &[u8], i: usize) -> Token {
    scan(s, i, b":?\"'/-", false)
}

/// ext/pdo_sqlite's scanner (`sqlite_sql_parser.re`): also skips
/// `` `identifiers` `` and `[identifiers]`.
pub fn sqlite_scanner(s: &[u8], i: usize) -> Token {
    scan(s, i, b":?\"'`/[-", true)
}

/// Result of [`parse_params`].
pub enum Parsed {
    /// Nothing to rewrite (`0`).
    Unchanged,
    /// The rewritten query (`1`).
    Rewritten(PhpStr),
    /// An error was raised (`-1`).
    Error,
}

struct Placeholder {
    pos: usize,
    len: usize,
    quoted: Option<PhpStr>,
    /// `PDO_PARSER_BINDNO_ESCAPED_CHAR` is `-1`.
    bindno: i64,
}

fn map_update(map: &mut Vec<(ParamKey, PhpStr)>, key: ParamKey, v: PhpStr) {
    match map.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = v,
        None => map.push((key, v)),
    }
}

/// `zend_strpprintf(0, tmpl, n)` for the `%d` rewrite templates.
fn format_template(tmpl: &str, n: i64) -> PhpStr {
    PhpStr::from(tmpl.replacen("%d", &n.to_string(), 1))
}

/// `pdo_parse_params`: scans `query` for placeholders and, for emulated
/// prepares, substitutes the quoted bound values; for drivers with the
/// other placeholder style, rewrites the placeholders.
pub fn parse_params(vm: &mut Vm, stmt: &Stmt, dbh: &Dbh, query: &PhpStr) -> PResult<Parsed> {
    let methods = dbh.methods();
    let scanner = methods
        .as_ref()
        .and_then(|m| m.scanner())
        .unwrap_or(default_scanner);
    let q = query.as_bytes();
    let supports = stmt.supports_placeholders.get();

    let mut placeholders: Vec<Placeholder> = Vec::new();
    let mut query_type = PLACEHOLDER_NONE;
    let mut bindno: i64 = 0;
    let mut escapes = 0;
    let mut custom_quote: Option<(usize, usize)> = None;
    let mut cur = 0;
    loop {
        let tok = scanner(q, cur);
        if tok.kind == TokenKind::Eoi {
            break;
        }
        let start = cur;
        cur = tok.end;
        let mut forced_placeholder = false;
        if let Some((pos, len)) = custom_quote {
            if tok.kind == TokenKind::CustomQuote
                && len == tok.end - start
                && q[start..tok.end] == q[pos..pos + len]
            {
                custom_quote = None;
            } else if tok.kind == TokenKind::EscapedQuestion {
                vm.deprecated(
                    "Escaping question marks inside dollar quoted strings is not required anymore and is deprecated",
                )?;
                forced_placeholder = true;
            }
            if !forced_placeholder {
                continue;
            }
        }
        if !forced_placeholder {
            if tok.kind == TokenKind::CustomQuote {
                custom_quote = Some((start, tok.end - start));
                continue;
            }
            match tok.kind {
                TokenKind::Bind | TokenKind::BindPos | TokenKind::EscapedQuestion => {}
                _ => continue,
            }
            if tok.kind == TokenKind::EscapedQuestion && supports == PLACEHOLDER_POSITIONAL {
                // Escaped question marks are unsupported: plain text.
                continue;
            }
            if tok.kind == TokenKind::Bind {
                if start > 0 && q[start - 1].is_ascii_alphanumeric() {
                    continue;
                }
                query_type |= PLACEHOLDER_NAMED;
            } else if tok.kind == TokenKind::BindPos {
                query_type |= PLACEHOLDER_POSITIONAL;
            }
        }
        let escaped = tok.kind == TokenKind::EscapedQuestion;
        placeholders.push(Placeholder {
            pos: start,
            len: tok.end - start,
            quoted: escaped.then(|| PhpStr::from("?")),
            bindno: if escaped {
                -1
            } else {
                bindno += 1;
                bindno - 1
            },
        });
        if escaped {
            escapes += 1;
        }
    }

    if query_type == PLACEHOLDER_NAMED | PLACEHOLDER_POSITIONAL {
        raise_impl_error(
            vm,
            dbh,
            Some(stmt),
            "HY093",
            Some("mixed named and positional parameters"),
        )?;
        return Ok(Parsed::Error);
    }

    let params = stmt.bound_params.borrow().clone();
    let mut safe = false;
    if supports == PLACEHOLDER_NONE {
        if let Some(params) = &params {
            if bindno as usize != params.len() {
                let mut ok = false;
                if query_type != PLACEHOLDER_POSITIONAL && bindno as usize > params.len() {
                    ok = placeholders.iter().all(|p| {
                        params
                            .get(&ParamKey::Str(PhpStr::from_bytes(&q[p.pos..p.pos + p.len])))
                            .is_some()
                    });
                }
                if !ok {
                    raise_impl_error(
                        vm,
                        dbh,
                        Some(stmt),
                        "HY093",
                        Some("number of bound variables does not match number of tokens"),
                    )?;
                    return Ok(Parsed::Error);
                }
                safe = true;
            }
        }
    }

    if placeholders.is_empty() {
        return Ok(Parsed::Unchanged);
    }

    if !safe {
        if supports == query_type && stmt.named_rewrite_template.get().is_none() {
            if escapes > 0 {
                return Ok(Parsed::Rewritten(rewrite(q, &placeholders)));
            }
            return Ok(Parsed::Unchanged);
        }
        if query_type == PLACEHOLDER_NAMED && stmt.named_rewrite_template.get().is_some() {
            // Pretend the query was positional to reach the named rewrite.
            query_type = PLACEHOLDER_POSITIONAL;
        }
    }

    if supports == PLACEHOLDER_NONE {
        let Some(params) = params else {
            return Ok(Parsed::Rewritten(rewrite(q, &placeholders)));
        };
        for plc in placeholders.iter_mut() {
            if plc.bindno == -1 || query_type == PLACEHOLDER_NONE {
                continue;
            }
            let param = if query_type == PLACEHOLDER_POSITIONAL {
                params.get(&ParamKey::Int(plc.bindno))
            } else {
                params.get(&ParamKey::Str(PhpStr::from_bytes(
                    &q[plc.pos..plc.pos + plc.len],
                )))
            };
            let Some(param) = param.cloned() else {
                raise_impl_error(
                    vm,
                    dbh,
                    Some(stmt),
                    "HY093",
                    Some("parameter was not defined"),
                )?;
                return Ok(Parsed::Error);
            };
            let value = param.value();
            let Some(m) = methods.as_ref().filter(|m| m.supports_quote()) else {
                plc.quoted = Some(match value {
                    Value::String(s) => s,
                    other => vm.to_php_string(&other)?,
                });
                continue;
            };
            if param.param_type == PARAM_LOB as i32 && matches!(value, Value::Resource(_)) {
                let Value::Resource(r) = &value else {
                    unreachable!()
                };
                if crate::standard::streams::handle_of(r).is_none() {
                    raise_impl_error(
                        vm,
                        dbh,
                        Some(stmt),
                        "HY105",
                        Some("Expected a stream resource"),
                    )?;
                    return Ok(Parsed::Error);
                }
                let buf = crate::standard::streams::read_to_end(vm, r)?.unwrap_or_default();
                match m.quote(vm, dbh, &PhpStr::from_vec(buf), param.param_type as i64)? {
                    Some(qs) => plc.quoted = Some(qs),
                    None => {
                        stmt.error_code.set(dbh.error_code.get());
                        return Ok(Parsed::Error);
                    }
                }
                continue;
            }
            let param_type = if value.is_null() {
                PARAM_NULL as i32
            } else {
                param.param_type
            };
            plc.quoted = Some(match param_type {
                t if t == PARAM_BOOL as i32 => {
                    PhpStr::from(if vm.to_bool(&value)? { "1" } else { "0" })
                }
                t if t == PARAM_INT as i32 => PhpStr::from(vm.to_int(&value)?.to_string()),
                t if t == PARAM_NULL as i32 => PhpStr::from("NULL"),
                _ => {
                    let buf = match vm.to_php_string(&value) {
                        Ok(b) => b,
                        Err(e) => {
                            stmt.error_code.set(dbh.error_code.get());
                            return Err(e);
                        }
                    };
                    match m.quote(vm, dbh, &buf, param_type as i64)? {
                        Some(qs) => qs,
                        None => {
                            stmt.error_code.set(dbh.error_code.get());
                            return Ok(Parsed::Error);
                        }
                    }
                }
            });
        }
        return Ok(Parsed::Rewritten(rewrite(q, &placeholders)));
    }

    if query_type == PLACEHOLDER_POSITIONAL {
        // Rewrite `?` to `:pdoN` (or the driver's template).
        let tmpl = stmt.named_rewrite_template.get().unwrap_or(":pdo%d");
        let mut bind_no = 1;
        let mut map = stmt.bound_param_map.borrow().clone().unwrap_or_default();
        for plc in placeholders.iter_mut() {
            if plc.bindno == -1 {
                continue;
            }
            let name = PhpStr::from_bytes(&q[plc.pos..plc.pos + plc.len]);
            let existing = map
                .iter()
                .find(|(k, _)| *k == ParamKey::Str(name.clone()))
                .map(|(_, v)| v.clone());
            let (idx, skip_map) = match existing {
                Some(p) if name.as_bytes() != b"?" => (p, true),
                _ => {
                    let s = format_template(tmpl, bind_no);
                    bind_no += 1;
                    (s, false)
                }
            };
            plc.quoted = Some(idx.clone());
            if !skip_map && stmt.named_rewrite_template.get().is_some() {
                map_update(&mut map, ParamKey::Str(name), idx.clone());
            }
            map_update(&mut map, ParamKey::Int(plc.bindno), idx);
        }
        *stmt.bound_param_map.borrow_mut() = Some(map);
        return Ok(Parsed::Rewritten(rewrite(q, &placeholders)));
    }

    // Rewrite `:name` to `?`.
    let mut map = stmt.bound_param_map.borrow().clone().unwrap_or_default();
    for plc in placeholders.iter_mut() {
        let name = PhpStr::from_bytes(&q[plc.pos..plc.pos + plc.len]);
        map_update(&mut map, ParamKey::Int(plc.bindno), name);
        plc.quoted = Some(PhpStr::from("?"));
    }
    *stmt.bound_param_map.borrow_mut() = Some(map);
    Ok(Parsed::Rewritten(rewrite(q, &placeholders)))
}

/// Builds the query with each placeholder replaced by its `quoted` text.
fn rewrite(q: &[u8], placeholders: &[Placeholder]) -> PhpStr {
    let mut out = Vec::with_capacity(q.len());
    let mut ptr = 0;
    for plc in placeholders {
        out.extend_from_slice(&q[ptr..plc.pos]);
        match &plc.quoted {
            Some(s) => out.extend_from_slice(s),
            None => out.extend_from_slice(&q[plc.pos..plc.pos + plc.len]),
        }
        ptr = plc.pos + plc.len;
    }
    out.extend_from_slice(&q[ptr..]);
    PhpStr::from_vec(out)
}

/// Placeholder positions found by `scanner` (for tests and drivers).
pub fn tokens(scanner: fn(&[u8], usize) -> Token, q: &[u8]) -> Vec<(TokenKind, Vec<u8>)> {
    let mut out = Vec::new();
    let mut cur = 0;
    loop {
        let t = scanner(q, cur);
        if t.kind == TokenKind::Eoi {
            break;
        }
        out.push((t.kind, q[cur..t.end].to_vec()));
        cur = t.end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(scanner: fn(&[u8], usize) -> Token, q: &str) -> Vec<(TokenKind, String)> {
        tokens(scanner, q.as_bytes())
            .into_iter()
            .filter(|(k, _)| *k != TokenKind::Text)
            .map(|(k, s)| (k, String::from_utf8(s).unwrap()))
            .collect()
    }

    #[test]
    fn default_scanner_finds_placeholders() {
        use TokenKind::*;
        assert_eq!(
            kinds(default_scanner, "SELECT * FROM t WHERE a = ? AND b = :name"),
            vec![(BindPos, "?".into()), (Bind, ":name".into())]
        );
        // Quoted strings, comments and `::`/`??` are text.
        assert!(
            kinds(
                default_scanner,
                "SELECT '?', \"?\", a::int, ?? /* ? */ -- ?\n"
            )
            .is_empty()
        );
        assert_eq!(
            kinds(default_scanner, "SELECT 'it''s ?' , ?"),
            vec![(BindPos, "?".into())]
        );
        // An unterminated quote is a single special character.
        assert_eq!(
            kinds(default_scanner, "SELECT '?"),
            vec![(BindPos, "?".into())]
        );
        // A trailing line comment or open block comment ends the scan.
        assert!(kinds(default_scanner, "SELECT 1 -- ?").is_empty());
        assert!(kinds(default_scanner, "SELECT 1 /* ?").is_empty());
        // A NUL byte ends the query.
        assert!(kinds(default_scanner, "SELECT 1\0?").is_empty());
    }

    #[test]
    fn sqlite_scanner_skips_identifiers() {
        use TokenKind::*;
        assert_eq!(
            kinds(sqlite_scanner, "SELECT * FROM t WHERE `a``?` = ?"),
            vec![(BindPos, "?".into())]
        );
        assert_eq!(
            kinds(sqlite_scanner, "SELECT * FROM t WHERE [a`?] = ?"),
            vec![(BindPos, "?".into())]
        );
        // re2c's longest match runs to the last quote of the query.
        assert!(kinds(sqlite_scanner, "SELECT * FROM t WHERE '?' IN (?, '?')").is_empty());
    }

    #[test]
    fn templates() {
        assert_eq!(format_template(":pdo%d", 3).as_bytes(), b":pdo3");
        assert_eq!(format_template("$%d", 12).as_bytes(), b"$12");
    }
}
