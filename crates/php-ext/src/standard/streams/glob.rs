//! main/php_glob.c, main/php_glob.h and main/charclass.h: the BSD `glob(3)` that php-src bundles
//! and uses on every platform (it is built without `PHP_SYSTEM_GLOB`).
//!
//! The port keeps the C structure (`globexp1`/`globexp2` brace expansion, `globtilde`, `glob0`
//! pattern compilation, the mutually recursive `glob1`/`glob2`/`glob3` directory walk, `match`,
//! `globextend`) and its 16-bit `Char` pattern encoding (`M_PROTECT` for backslash-quoted
//! characters, `M_QUOTE` for compiled meta characters), so quirks of the C code carry over:
//!
//! * `GLOB_ERR` alone changes nothing: `glob3` only consults it when an error callback is
//!   installed, and PHP always passes `NULL` ([`php_glob_ex`] accepts a callback).
//! * With `GLOB_BRACE`, unbalanced braces glob only the part of the pattern before the brace
//!   (`globexp2` hands its prefix buffer to `glob0`), and the result is sorted per alternative.
//! * `[[:alnum:]]` compiles to the class index 0, which doubles as the end-of-pattern marker, so
//!   anything after such a bracket expression is ignored.
//! * A path that would overflow the `PATH_MAX` buffer aborts the whole glob with the bare code
//!   `1`, even for a directory entry that would not have matched.
//! * `GLOB_NOCHECK` (and `GLOB_NOMAGIC`) return the pattern with its backslash escapes removed.
//!
//! Differences from the C code: `GLOB_ALTDIRFUNC`, `GLOB_DOOFFS` and `GLOB_KEEPSTAT` are
//! accepted but ignored (no alternate directory functions, offsets or stat vectors); `~user`
//! is resolved from `/etc/passwd` only (no NSS / Directory Services) and a plain `~` with `$HOME`
//! unset is left unexpanded (no `getpwuid`); a bracket expression whose closing `]` was eaten by
//! a `[:class:]` ends at the end of the pattern (C reads past the end of its buffer there).
//! Directory entries are read with [`std::fs::read_dir`], with `.` and `..` reported first the
//! way `readdir()` does, so `.*` matches them.
//!
//! # Callers
//!
//! `glob()` (ext/standard/dir.c):
//! 1. Reject patterns with NUL bytes (`Z_PARAM_PATH`), then a pattern of [`MAXPATHLEN`] or more
//!    bytes: warning "Pattern exceeds the maximum allowed length of %d characters", `false`.
//! 2. `(GLOB_AVAILABLE_FLAGS & flags) != flags`: warning "At least one of the passed flags is
//!    invalid or not supported on this platform", `false`.
//! 3. Call `php_glob(pattern, flags & GLOB_FLAGMASK)`. `Err((GLOB_NOMATCH, _))` returns an empty
//!    array, any other `Err` returns `false`.
//! 4. For each path: skip it (and remember that one was skipped) when open_basedir rejects it;
//!    with `GLOB_ONLYDIR` (emulated, the bundled glob does not know it) skip paths for which
//!    [`glob_is_dir`] is false ([`glob_onlydir_filter`] does this for a whole list).
//! 5. If open_basedir skipped a path and nothing is left, return `false`.
//!
//! The `glob://` wrapper (main/streams/glob_wrapper.c) calls `php_glob(pattern, 0)` (its flags
//! field is still zero at that point), fails the open on any `Err` other than `GLOB_NOMATCH`,
//! filters through open_basedir unless `STREAM_DISABLE_OPEN_BASEDIR`, and yields the part of each
//! path after the last `/` from `readdir()`.

use std::borrow::Cow;
use std::fs;
use std::path::Path;

/// Append to output from previous call ([`php_glob_ex`] only).
pub const GLOB_APPEND: i32 = 0x0001;
/// Use `gl_offs` (accepted, ignored).
pub const GLOB_DOOFFS: i32 = 0x0002;
/// Return on error (only together with an error callback, see [`php_glob_ex`]).
pub const GLOB_ERR: i32 = 0x0004;
/// Append `/` to matching directories.
pub const GLOB_MARK: i32 = 0x0008;
/// Return the pattern itself if nothing matches.
pub const GLOB_NOCHECK: i32 = 0x0010;
/// Don't sort.
pub const GLOB_NOSORT: i32 = 0x0020;
/// Use alternately specified directory functions (accepted, ignored).
pub const GLOB_ALTDIRFUNC: i32 = 0x0040;
/// Expand braces à la csh.
pub const GLOB_BRACE: i32 = 0x0080;
/// Set in the result flags when the pattern had globbing characters.
pub const GLOB_MAGCHAR: i32 = 0x0100;
/// `GLOB_NOCHECK` without magic characters (csh).
pub const GLOB_NOMAGIC: i32 = 0x0200;
/// Quote special characters with `\` (the bundled glob always does unless `GLOB_NOESCAPE`).
pub const GLOB_QUOTE: i32 = 0x0400;
/// Expand `~` and `~user`.
pub const GLOB_TILDE: i32 = 0x0800;
/// Disable backslash escaping.
pub const GLOB_NOESCAPE: i32 = 0x1000;
/// Limit memory, `stat()` and `readdir()` use (`GLOB_LIMIT_*`).
pub const GLOB_LIMIT: i32 = 0x2000;
/// Retain stat data (accepted, ignored).
pub const GLOB_KEEPSTAT: i32 = 0x4000;
/// PHP's own flag, emulated by the caller (see [`glob_onlydir_filter`]).
pub const GLOB_ONLYDIR: i32 = 1 << 30;
/// Mask applied to the user's flags before calling [`php_glob`].
pub const GLOB_FLAGMASK: i32 = !GLOB_ONLYDIR;
/// The flags `glob()` accepts.
pub const GLOB_AVAILABLE_FLAGS: i32 =
    GLOB_BRACE | GLOB_MARK | GLOB_NOSORT | GLOB_NOCHECK | GLOB_NOESCAPE | GLOB_ERR | GLOB_ONLYDIR;

/// Malloc call failed (or a `GLOB_LIMIT` limit was hit).
pub const GLOB_NOSPACE: i32 = -1;
/// Unignored error.
pub const GLOB_ABORTED: i32 = -2;
/// No match and `GLOB_NOCHECK` not set.
pub const GLOB_NOMATCH: i32 = -3;
/// Function not supported.
pub const GLOB_NOSYS: i32 = -4;

/// `GLOB_LIMIT`: bytes of result paths plus the path vector.
pub const GLOB_LIMIT_MALLOC: usize = 65536;
/// `GLOB_LIMIT`: `lstat()` calls.
pub const GLOB_LIMIT_STAT: usize = 2048;
/// `GLOB_LIMIT`: directory entries read.
pub const GLOB_LIMIT_READDIR: usize = 16384;

/// The platform's `PATH_MAX`, the size of every pattern and path buffer in php_glob.c.
pub const PATH_MAX: usize = if cfg!(any(target_os = "linux", target_os = "android")) {
    4096
} else {
    1024
};
/// PHP's `MAXPATHLEN` (`PATH_MAX` on POSIX systems).
pub const MAXPATHLEN: usize = PATH_MAX;

/// `php_glob_t` minus the C plumbing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobResult {
    /// `gl_pathv`: the matched paths (`gl_pathc` is its length).
    pub paths: Vec<Vec<u8>>,
    /// `gl_matchc`: paths that matched the pattern (not counting `GLOB_NOCHECK` fallbacks).
    pub matchc: usize,
    /// `gl_flags`: the flags passed in, plus `GLOB_MAGCHAR` when the pattern had meta characters.
    pub flags: i32,
}

/// `php_glob(pattern, flags, NULL, &globbuf)`.
///
/// `Ok` when the C function returns 0. Otherwise `Err` with the return code (`GLOB_NOMATCH`,
/// `GLOB_NOSPACE`, `GLOB_ABORTED`, or `1` when a path overflows the `PATH_MAX` buffer) and the
/// partial result. The pattern is a C string: anything from the first NUL byte on is ignored.
pub fn php_glob(pattern: &[u8], flags: i32) -> Result<GlobResult, (i32, GlobResult)> {
    let mut result = GlobResult::default();
    match php_glob_ex(pattern, flags, None, &mut result) {
        0 => Ok(result),
        code => Err((code, result)),
    }
}

/// `gl_errfunc`: called with a directory path and `errno` when the directory cannot be opened.
pub type GlobErrFunc<'f> = dyn FnMut(&[u8], i32) -> i32 + 'f;

/// The full `php_glob(pattern, flags, errfunc, pglob)`: returns the C return code.
///
/// A nonzero return from `errfunc`, or `GLOB_ERR` in `flags` once `errfunc` has been called,
/// aborts the glob with `GLOB_ABORTED`. Without `GLOB_APPEND` the previous contents of `pglob`
/// are discarded.
pub fn php_glob_ex(
    pattern: &[u8],
    flags: i32,
    errfunc: Option<&mut GlobErrFunc<'_>>,
    pglob: &mut GlobResult,
) -> i32 {
    // php_glob() forces GLOB_NOESCAPE on Windows because `\` is a path separator there.
    let flags = if cfg!(windows) {
        flags | GLOB_NOESCAPE
    } else {
        flags
    };
    if flags & GLOB_APPEND == 0 {
        pglob.paths.clear();
    }
    pglob.flags = flags & !GLOB_MAGCHAR;
    pglob.matchc = 0;

    let pattern = pattern.split(|&b| b == 0).next().unwrap_or_default();
    if pattern.len() >= PATH_MAX {
        return GLOB_NOMATCH;
    }

    let mut patbuf: Vec<Char> = Vec::with_capacity(pattern.len() + 1);
    if flags & GLOB_NOESCAPE != 0 {
        patbuf.extend(pattern.iter().map(|&b| Char::from(b)));
    } else {
        // Protect the quoted characters; a trailing backslash quotes itself.
        let mut bytes = pattern.iter();
        while let Some(&c) = bytes.next() {
            if c == b'\\' {
                let quoted = bytes.next().copied().unwrap_or(b'\\');
                patbuf.push(Char::from(quoted) | M_PROTECT);
            } else {
                patbuf.push(Char::from(c));
            }
        }
    }
    patbuf.push(EOS);

    let mut glob = Globber {
        pglob,
        errfunc,
        limit: Limits::default(),
    };
    if flags & GLOB_BRACE != 0 {
        glob.globexp1(&patbuf)
    } else {
        glob.glob0(&patbuf)
    }
}

/// `dir.c`'s `GLOB_ONLYDIR` test: `VCWD_STAT()` (following symlinks) reports a directory.
pub fn glob_is_dir(path: &[u8]) -> bool {
    fs::metadata(os_path(path)).is_ok_and(|m| m.is_dir())
}

/// Keeps only the paths that pass [`glob_is_dir`], as `glob()` does for `GLOB_ONLYDIR`.
pub fn glob_onlydir_filter(paths: &mut Vec<Vec<u8>>) {
    paths.retain(|p| glob_is_dir(p));
}

/// A pattern character: a byte, optionally with `M_PROTECT` (quoted) or `M_QUOTE` (meta) set.
/// Patterns are kept NUL (`EOS`) terminated like the C buffers.
type Char = u16;

const EOS: Char = 0;
const CIRCUMFLEX: Char = b'^' as Char;
const COLON: Char = b':' as Char;
const COMMA: Char = b',' as Char;
const DOT: Char = b'.' as Char;
const LBRACE: Char = b'{' as Char;
const LBRACKET: Char = b'[' as Char;
const NOT: Char = b'!' as Char;
const QUESTION: Char = b'?' as Char;
const RANGE: Char = b'-' as Char;
const RBRACE: Char = b'}' as Char;
const RBRACKET: Char = b']' as Char;
const SLASH: Char = b'/' as Char;
const STAR: Char = b'*' as Char;
const TILDE: Char = b'~' as Char;

/// `DEFAULT_SLASH`.
const SEP: u8 = if cfg!(windows) { b'\\' } else { b'/' };

// M_MASK is 0xffff, the whole `Char`, so masking with it is left out below.
const M_QUOTE: Char = 0x8000;
const M_PROTECT: Char = 0x4000;
const M_ASCII: Char = 0x00ff;

const M_ALL: Char = meta(b'*');
const M_END: Char = meta(b']');
const M_NOT: Char = meta(b'!');
const M_ONE: Char = meta(b'?');
const M_RNG: Char = meta(b'-');
const M_SET: Char = meta(b'[');
const M_CLASS: Char = meta(b':');

const fn meta(c: u8) -> Char {
    c as Char | M_QUOTE
}

/// `CHAR()`: strips `M_PROTECT` / `M_QUOTE`.
const fn plain(c: Char) -> Char {
    c & M_ASCII
}

const fn ismeta(c: Char) -> bool {
    c & M_QUOTE != 0
}

fn is_slash(c: Char) -> bool {
    c == SLASH || (cfg!(windows) && c == b'\\' as Char)
}

/// `*(s + i)`, reading `EOS` past the end of the buffer.
fn at(s: &[Char], i: usize) -> Char {
    s.get(i).copied().unwrap_or(EOS)
}

/// `g_strchr(s + from, c)`: index of the first `c` at or after `from`, before the `EOS`.
fn g_strchr(s: &[Char], from: usize, c: Char) -> Option<usize> {
    let mut i = from;
    loop {
        let x = at(s, i);
        if x == c {
            return Some(i);
        }
        if x == EOS {
            return None;
        }
        i += 1;
    }
}

/// charclass.h `cclasses`, in order: a compiled `[:name:]` stores its index here.
const CCLASSES: [&str; 12] = [
    "alnum", "alpha", "blank", "cntrl", "digit", "graph", "lower", "print", "punct", "space",
    "upper", "xdigit",
];

/// The `<ctype.h>` predicate of class `idx` in the "C" locale (PHP 8 never inherits LC_CTYPE).
fn isctype(idx: usize, k: Char) -> bool {
    let Ok(k) = u8::try_from(k) else {
        return false;
    };
    match idx {
        0 => k.is_ascii_alphanumeric(),
        1 => k.is_ascii_alphabetic(),
        2 => k == b' ' || k == b'\t',
        3 => k.is_ascii_control(),
        4 => k.is_ascii_digit(),
        5 => k.is_ascii_graphic(),
        6 => k.is_ascii_lowercase(),
        7 => k == b' ' || k.is_ascii_graphic(),
        8 => k.is_ascii_punctuation(),
        // isspace() includes \v, which is_ascii_whitespace() does not.
        9 => k == b' ' || (b'\t'..=b'\r').contains(&k),
        10 => k.is_ascii_uppercase(),
        11 => k.is_ascii_hexdigit(),
        _ => false,
    }
}

#[derive(Default)]
struct Limits {
    malloc: usize,
    stat: usize,
    readdir: usize,
}

struct Globber<'a, 'f> {
    pglob: &'a mut GlobResult,
    errfunc: Option<&'a mut GlobErrFunc<'f>>,
    limit: Limits,
}

impl Globber<'_, '_> {
    fn flag(&self, flag: i32) -> bool {
        self.pglob.flags & flag != 0
    }

    /// `globexp1`: expand the first brace of `pattern`, or glob it when there is none.
    fn globexp1(&mut self, pattern: &[Char]) -> i32 {
        // Protect a single {}, for find(1), like csh.
        if at(pattern, 0) == LBRACE && at(pattern, 1) == RBRACE && at(pattern, 2) == EOS {
            return self.glob0(pattern);
        }
        match g_strchr(pattern, 0, LBRACE) {
            Some(brace) => self.globexp2(pattern, brace),
            None => self.glob0(pattern),
        }
    }

    /// `globexp2`: glob every alternative of the brace at `brace`.
    fn globexp2(&mut self, pattern: &[Char], brace: usize) -> i32 {
        // Copy the part up to the brace.
        let mut patbuf: Vec<Char> = pattern[..brace].to_vec();
        let ls = patbuf.len();
        let ptr = brace + 1;

        // Find the balanced brace, ignoring everything between [].
        let mut depth = 0usize;
        let mut pe = ptr;
        while at(pattern, pe) != EOS {
            match at(pattern, pe) {
                LBRACKET => {
                    let open = pe;
                    pe += 1;
                    while at(pattern, pe) != RBRACKET && at(pattern, pe) != EOS {
                        pe += 1;
                    }
                    if at(pattern, pe) == EOS {
                        // No matching RBRACKET: ignore it and just look for RBRACE.
                        pe = open;
                    }
                }
                LBRACE => depth += 1,
                RBRACE => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            pe += 1;
        }

        // Non matching braces: glob the prefix (php_glob.c passes its prefix buffer here).
        if depth != 0 || at(pattern, pe) == EOS {
            patbuf.push(EOS);
            return self.glob0(&patbuf);
        }

        let end = g_strchr(pattern, pe, EOS).unwrap_or(pattern.len());
        let mut pl = ptr;
        let mut pm = ptr;
        while pm <= pe {
            match at(pattern, pm) {
                LBRACKET => {
                    let open = pm;
                    pm += 1;
                    while at(pattern, pm) != RBRACKET && at(pattern, pm) != EOS {
                        pm += 1;
                    }
                    if at(pattern, pm) == EOS {
                        pm = open;
                    }
                }
                LBRACE => depth += 1,
                RBRACE if depth > 0 => depth -= 1,
                COMMA if depth > 0 => {}
                RBRACE | COMMA => {
                    // The current alternative followed by the rest after the closing brace.
                    patbuf.truncate(ls);
                    patbuf.extend_from_slice(&pattern[pl..pm]);
                    patbuf.extend_from_slice(&pattern[pe + 1..end]);
                    patbuf.push(EOS);
                    let rv = self.globexp1(&patbuf);
                    if rv != 0 && rv != GLOB_NOMATCH {
                        return rv;
                    }
                    pl = pm + 1;
                }
                _ => {}
            }
            pm += 1;
        }
        0
    }

    /// `glob0`: compile `pattern`, glob it, then apply `GLOB_NOCHECK` / `GLOB_NOMAGIC` and sort.
    fn glob0(&mut self, pattern: &[Char]) -> i32 {
        let qpat = globtilde(pattern, self.pglob.flags, &home_dir);
        let oldpathc = self.pglob.paths.len();
        let mut buf: Vec<Char> = Vec::with_capacity(qpat.len() + 1);
        let mut q = 0;

        loop {
            let c = at(&qpat, q);
            q += 1;
            match c {
                EOS => break,
                LBRACKET => {
                    let mut c = at(&qpat, q);
                    // Not in OpenBSD's glob(): like fnmatch(), a leading '^' negates too.
                    if c == CIRCUMFLEX {
                        c = NOT;
                    }
                    if c == NOT {
                        q += 1;
                    }
                    if at(&qpat, q) == EOS || g_strchr(&qpat, q + 1, RBRACKET).is_none() {
                        buf.push(LBRACKET);
                        if c == NOT {
                            q -= 1;
                        }
                        continue;
                    }
                    buf.push(M_SET);
                    if c == NOT {
                        buf.push(M_NOT);
                    }
                    let mut c = at(&qpat, q);
                    q += 1;
                    loop {
                        if c == LBRACKET && at(&qpat, q) == COLON {
                            let mut err;
                            loop {
                                err = g_charclass(&qpat, &mut q, &mut buf);
                                if err != 0 {
                                    break;
                                }
                                c = at(&qpat, q);
                                q += 1;
                                if !(c == LBRACKET && at(&qpat, q) == COLON) {
                                    break;
                                }
                            }
                            if err == -1 && !self.flag(GLOB_NOCHECK) {
                                return GLOB_NOMATCH;
                            }
                            if c == RBRACKET {
                                break;
                            }
                        }
                        if c == EOS {
                            // A class consumed the `]` the bracket check found; C runs off
                            // the end of its buffer here, we end the set with the pattern.
                            q -= 1;
                            break;
                        }
                        buf.push(plain(c));
                        let hi = at(&qpat, q + 1);
                        if at(&qpat, q) == RANGE && hi != RBRACKET {
                            buf.push(M_RNG);
                            buf.push(plain(hi));
                            q += 2;
                        }
                        c = at(&qpat, q);
                        q += 1;
                        if c == RBRACKET {
                            break;
                        }
                    }
                    self.pglob.flags |= GLOB_MAGCHAR;
                    buf.push(M_END);
                }
                QUESTION => {
                    self.pglob.flags |= GLOB_MAGCHAR;
                    buf.push(M_ONE);
                }
                STAR => {
                    self.pglob.flags |= GLOB_MAGCHAR;
                    // Collapse adjacent stars to one, to avoid exponential behavior.
                    if buf.last() != Some(&M_ALL) {
                        buf.push(M_ALL);
                    }
                }
                _ => buf.push(plain(c)),
            }
        }
        buf.push(EOS);

        let err = self.glob1(&buf);
        if err != 0 {
            return err;
        }

        if self.pglob.paths.len() == oldpathc {
            if self.flag(GLOB_NOCHECK) || (self.flag(GLOB_NOMAGIC) && !self.flag(GLOB_MAGCHAR)) {
                // g_Ctoc() keeps the low byte, dropping M_PROTECT.
                let end = g_strchr(pattern, 0, EOS).unwrap_or(pattern.len());
                let path: Vec<u8> = pattern[..end].iter().map(|&c| c as u8).collect();
                return self.globextend(&path);
            }
            return GLOB_NOMATCH;
        }
        if !self.flag(GLOB_NOSORT) {
            // compare(): strcmp() byte order.
            self.pglob.paths[oldpathc..].sort_unstable();
        }
        0
    }

    fn glob1(&mut self, pattern: &[Char]) -> i32 {
        // A null pathname is invalid -- POSIX 1003.1 sect. 2.4.
        if at(pattern, 0) == EOS {
            return 0;
        }
        let mut path = Vec::with_capacity(PATH_MAX);
        self.glob2(&mut path, pattern, 0)
    }

    /// `glob2`: append literal segments of the compiled pattern (from `pi`) to `path` until the
    /// end, where the path is checked and added, or a segment with meta characters.
    /// `path.len()` plays the role of `pathend`.
    fn glob2(&mut self, path: &mut Vec<u8>, pattern: &[Char], mut pi: usize) -> i32 {
        let last = PATH_MAX - 1;
        let mut anymeta = false;
        loop {
            if at(pattern, pi) == EOS {
                if self.flag(GLOB_LIMIT) {
                    let n = self.limit.stat;
                    self.limit.stat += 1;
                    if n >= GLOB_LIMIT_STAT {
                        return GLOB_NOSPACE;
                    }
                }
                let Ok(meta) = fs::symlink_metadata(os_path(path)) else {
                    return 0;
                };
                if self.flag(GLOB_MARK)
                    && path.last() != Some(&SEP)
                    && (meta.is_dir() || (meta.file_type().is_symlink() && glob_is_dir(path)))
                {
                    if path.len() + 1 > last {
                        return 1;
                    }
                    path.push(SEP);
                }
                self.pglob.matchc += 1;
                return self.globextend(path);
            }

            // Find the end of the next segment, checking it fits in the path buffer.
            let pathend = path.len();
            let mut p = pi;
            while at(pattern, p) != EOS && !is_slash(at(pattern, p)) {
                if ismeta(at(pattern, p)) {
                    anymeta = true;
                }
                if pathend + (p - pi) + 1 > last {
                    return 1;
                }
                p += 1;
            }

            if anymeta {
                return self.glob3(path, pattern, pi, p);
            }
            // No expansion, do the next segment.
            path.extend(pattern[pi..p].iter().map(|&c| c as u8));
            pi = p;
            while is_slash(at(pattern, pi)) {
                if path.len() + 1 > last {
                    return 1;
                }
                path.push(pattern[pi] as u8);
                pi += 1;
            }
        }
    }

    /// `glob3`: match the entries of directory `path` against `pattern[pi..rest]` and recurse
    /// into `glob2` with the rest of the pattern for each match.
    fn glob3(&mut self, path: &mut Vec<u8>, pattern: &[Char], pi: usize, rest: usize) -> i32 {
        let last = PATH_MAX - 1;
        let pathend = path.len();
        if pathend > last {
            return 1;
        }

        let dir: &[u8] = if path.is_empty() { b"." } else { path };
        let entries = match fs::read_dir(os_path(dir)) {
            Ok(entries) => entries,
            Err(e) => {
                if let Some(errfunc) = self.errfunc.as_mut() {
                    let errno = e.raw_os_error().unwrap_or(0);
                    if errfunc(path, errno) != 0 || self.pglob.flags & GLOB_ERR != 0 {
                        return GLOB_ABORTED;
                    }
                }
                return 0;
            }
        };

        // readdir() reports "." and ".." too; read_dir() leaves them out.
        let names = [b".".to_vec(), b"..".to_vec()]
            .into_iter()
            .map(Ok)
            .chain(entries.map(|entry| entry.map(|e| os_bytes(e.file_name()))));

        let mut err = 0;
        for name in names {
            // A readdir() error ends the scan like the end of the directory.
            let Ok(name) = name else { break };
            if self.flag(GLOB_LIMIT) {
                let n = self.limit.readdir;
                self.limit.readdir += 1;
                if n >= GLOB_LIMIT_READDIR {
                    err = GLOB_NOSPACE;
                    break;
                }
            }

            // Initial DOT must be matched literally.
            if name.first() == Some(&b'.') && at(pattern, pi) != DOT {
                continue;
            }
            // The name (and its NUL) must fit before the last buffer slot, matched or not.
            if pathend + name.len() + 1 >= last {
                err = 1;
                break;
            }
            if !glob_match(&name, pattern, pi, rest) {
                continue;
            }
            path.extend_from_slice(&name);
            err = self.glob2(path, pattern, rest);
            path.truncate(pathend);
            if err != 0 {
                break;
            }
        }
        err
    }

    /// `globextend`: add `path` to the result, enforcing the `GLOB_LIMIT` memory limit.
    fn globextend(&mut self, path: &[u8]) -> i32 {
        let newn = 2 + self.pglob.paths.len();
        self.limit.malloc += path.len() + 1;
        self.pglob.paths.push(path.to_vec());
        if self.flag(GLOB_LIMIT)
            && newn * size_of::<*const u8>() + self.limit.malloc > GLOB_LIMIT_MALLOC
        {
            return GLOB_NOSPACE;
        }
        0
    }
}

/// `g_charclass`: compile the `[:name:]` at `pattern[*q - 1..]` (`*q` is at the first `:`).
/// Returns 1 when it is not a class, -1 for an unknown class name, 0 after compiling it.
fn g_charclass(pattern: &[Char], q: &mut usize, buf: &mut Vec<Char>) -> i32 {
    let start = *q + 1;
    let Some(colon) = g_strchr(pattern, start, COLON) else {
        return 1;
    };
    if at(pattern, colon + 1) != RBRACKET {
        return 1;
    }
    let name = &pattern[start..colon];
    let found = CCLASSES.iter().position(|cc| {
        cc.len() == name.len() && cc.bytes().zip(name).all(|(a, &b)| Char::from(a) == b)
    });
    let Some(idx) = found else {
        return -1;
    };
    buf.push(M_CLASS);
    buf.push(idx as Char);
    *q += name.len() + 3;
    0
}

/// `globtilde`: with `GLOB_TILDE`, replace a leading `~` or `~user` using `lookup` (which gets
/// the user name, empty for `~`). The result is truncated to `PATH_MAX - 1` characters.
fn globtilde<'p>(
    pattern: &'p [Char],
    flags: i32,
    lookup: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Cow<'p, [Char]> {
    if at(pattern, 0) != TILDE || flags & GLOB_TILDE == 0 {
        return Cow::Borrowed(pattern);
    }

    // The user name runs up to the end of the string or a '/' (stored as plain chars).
    let mut p = 1;
    let mut user = Vec::new();
    while at(pattern, p) != EOS && at(pattern, p) != SLASH {
        user.push(pattern[p] as u8);
        p += 1;
    }
    let Some(home) = lookup(&user) else {
        return Cow::Borrowed(pattern);
    };

    let eb = PATH_MAX - 1;
    let mut out: Vec<Char> = Vec::with_capacity(PATH_MAX);
    out.extend(
        home.iter()
            .take_while(|&&b| b != 0)
            .take(eb)
            .map(|&b| Char::from(b)),
    );
    // Append the rest of the pattern.
    while out.len() < eb {
        let c = at(pattern, p);
        p += 1;
        if c == EOS {
            break;
        }
        out.push(c);
    }
    out.push(EOS);
    Cow::Owned(out)
}

/// `$HOME` for `~`, the `/etc/passwd` home directory for `~user`.
fn home_dir(user: &[u8]) -> Option<Vec<u8>> {
    if user.is_empty() {
        return std::env::var_os("HOME").map(os_bytes);
    }
    passwd_home(&fs::read("/etc/passwd").ok()?, user)
}

/// The home directory (sixth field) of `user` in passwd(5) data.
fn passwd_home(passwd: &[u8], user: &[u8]) -> Option<Vec<u8>> {
    passwd
        .split(|&b| b == b'\n')
        .filter(|line| !line.starts_with(b"#"))
        .find_map(|line| {
            let mut fields = line.split(|&b| b == b':');
            if fields.next()? != user {
                return None;
            }
            fields.nth(4).map(<[u8]>::to_vec)
        })
}

/// `match`: whether `name` matches the compiled `pattern[pat..patend]`, without recursion
/// (Russ Cox's algorithm, as in php_glob.c). Bracket expressions may read past `patend` up to
/// their `M_END`, as in C.
fn glob_match(name: &[u8], pattern: &[Char], pat: usize, patend: usize) -> bool {
    let name_at = |i: usize| name.get(i).map_or(EOS, |&b| Char::from(b));
    let mut p = pat;
    let mut n = 0;
    let mut retry: Option<(usize, usize)> = None;

    loop {
        let matched = 'scan: {
            while p < patend {
                let c = at(pattern, p);
                p += 1;
                match c {
                    M_ALL => {
                        // Eat consecutive '*'.
                        while p < patend && at(pattern, p) == M_ALL {
                            p += 1;
                        }
                        if p == patend {
                            return true;
                        }
                        if name_at(n) == EOS {
                            return false;
                        }
                        retry = Some((p - 1, n + 1));
                    }
                    M_ONE => {
                        if name_at(n) == EOS {
                            break 'scan false;
                        }
                        n += 1;
                    }
                    M_SET => {
                        let k = name_at(n);
                        n += 1;
                        if k == EOS {
                            break 'scan false;
                        }
                        let negate = at(pattern, p) == M_NOT;
                        if negate {
                            p += 1;
                        }
                        let mut ok = false;
                        loop {
                            let c = at(pattern, p);
                            p += 1;
                            if c == M_END || p > pattern.len() {
                                break;
                            }
                            if c == M_CLASS {
                                if isctype(usize::from(at(pattern, p)), k) {
                                    ok = true;
                                }
                                p += 1;
                            }
                            if at(pattern, p) == M_RNG {
                                if c <= k && k <= at(pattern, p + 1) {
                                    ok = true;
                                }
                                p += 2;
                            } else if c == k {
                                ok = true;
                            }
                        }
                        if ok == negate {
                            break 'scan false;
                        }
                    }
                    _ => {
                        if name_at(n) != c {
                            break 'scan false;
                        }
                        n += 1;
                    }
                }
            }
            name_at(n) == EOS
        };
        if matched {
            return true;
        }
        // fail: retry from the last '*', letting it consume one more name character.
        let Some((rp, rn)) = retry else {
            return false;
        };
        p = rp;
        n = rn;
    }
}

#[cfg(unix)]
fn os_path(path: &[u8]) -> Cow<'_, Path> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(Path::new(std::ffi::OsStr::from_bytes(path)))
}

#[cfg(not(unix))]
fn os_path(path: &[u8]) -> Cow<'_, Path> {
    Cow::Owned(String::from_utf8_lossy(path).into_owned().into())
}

fn os_bytes(s: std::ffi::OsString) -> Vec<u8> {
    s.into_encoded_bytes()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Res = Result<Vec<String>, (i32, Vec<String>)>;

    /// A fresh directory under the system temp dir, removed on drop. Patterns are built from
    /// its absolute path, so the tests never depend on (or change) the working directory.
    struct TempDir {
        root: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            // Canonical, so no symlink (macOS /var -> /private/var) lengthens deep paths.
            let tmp = fs::canonicalize(std::env::temp_dir()).unwrap();
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let root = tmp.join(format!("php-glob-{tag}-{}-{seq}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            let dir = TempDir { root };
            assert!(
                !dir.base().iter().any(|b| b"*?[]{}\\~".contains(b)),
                "temp dir has glob meta characters"
            );
            dir
        }

        fn base(&self) -> Vec<u8> {
            os_bytes(self.root.clone().into_os_string())
        }

        fn touch(&self, rel: &str) {
            let path = self.root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"").unwrap();
        }

        fn mkdir(&self, rel: &str) {
            fs::create_dir_all(self.root.join(rel)).unwrap();
        }

        fn pattern(&self, rel: &str) -> Vec<u8> {
            let mut p = self.base();
            p.push(b'/');
            p.extend_from_slice(rel.as_bytes());
            p
        }

        /// The path relative to the root (or unchanged when outside it).
        fn rel(&self, path: &[u8]) -> String {
            let base = self.base();
            let rel = path
                .strip_prefix(&base[..])
                .and_then(|r| r.strip_prefix(b"/"))
                .unwrap_or(path);
            String::from_utf8_lossy(rel).into_owned()
        }

        fn rels(&self, result: &GlobResult) -> Vec<String> {
            result.paths.iter().map(|p| self.rel(p)).collect()
        }

        fn glob(&self, rel: &str, flags: i32) -> Res {
            match php_glob(&self.pattern(rel), flags) {
                Ok(r) => Ok(self.rels(&r)),
                Err((code, r)) => Err((code, self.rels(&r))),
            }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn ok(paths: &[&str]) -> Res {
        Ok(paths.iter().map(|p| p.to_string()).collect())
    }

    fn nomatch() -> Res {
        Err((GLOB_NOMATCH, vec![]))
    }

    fn sorted(r: Res) -> Res {
        r.map(|mut v| {
            v.sort();
            v
        })
    }

    #[test]
    fn glob_flag_values() {
        assert_eq!(GLOB_AVAILABLE_FLAGS, 0x4000_10bc);
        assert_eq!(GLOB_FLAGMASK & GLOB_ONLYDIR, 0);
        assert_eq!(GLOB_FLAGMASK & GLOB_BRACE, GLOB_BRACE);
    }

    #[test]
    fn glob_basic_star_and_question() {
        // ext/standard/tests/file/glob_basic.phpt
        let t = TempDir::new("basic");
        t.touch("wonder12345");
        t.touch("wonder.txt");
        t.touch("file.text");
        assert_eq!(
            t.glob("*", 0),
            ok(&["file.text", "wonder.txt", "wonder12345"])
        );
        assert_eq!(t.glob("*.txt", 0), ok(&["wonder.txt"]));
        assert_eq!(t.glob("*.t?t", 0), ok(&["wonder.txt"]));
        assert_eq!(t.glob("*.t*t", 0), ok(&["file.text", "wonder.txt"]));
        assert_eq!(t.glob("*.?", 0), nomatch());
        assert_eq!(t.glob("*.*", 0), ok(&["file.text", "wonder.txt"]));
    }

    #[test]
    fn glob_variation_patterns_and_flags() {
        // ext/standard/tests/file/glob_variation.phpt
        let t = TempDir::new("variation");
        t.mkdir("wonder");
        t.touch("wonder12345");
        t.touch("wonder;123456");
        let all = ["wonder", "wonder12345", "wonder;123456"];
        for pattern in ["*der*", "?onder*", "***der***"] {
            assert_eq!(t.glob(pattern, 0), ok(&all), "{pattern}");
            assert_eq!(
                t.glob(pattern, GLOB_MARK),
                ok(&["wonder/", "wonder12345", "wonder;123456"])
            );
            assert_eq!(sorted(t.glob(pattern, GLOB_NOSORT)), ok(&all));
            for flags in [GLOB_NOCHECK, GLOB_NOESCAPE, GLOB_ERR] {
                assert_eq!(t.glob(pattern, flags), ok(&all), "{pattern} {flags}");
            }
        }
        let two = ["wonder12345", "wonder;123456"];
        assert_eq!(t.glob("w*der?*", 0), ok(&two));
        assert_eq!(t.glob("w*der?*", GLOB_MARK), ok(&two));
        for pattern in ["*der5", "??onder*", "++onder*", "?wonder?", "wonder?"] {
            assert_eq!(t.glob(pattern, 0), nomatch(), "{pattern}");
            assert_eq!(t.glob(pattern, GLOB_MARK | GLOB_ERR), nomatch());
            let base = String::from_utf8(t.base()).unwrap();
            assert_eq!(
                php_glob(&t.pattern(pattern), GLOB_NOCHECK).map(|r| r.paths),
                Ok(vec![format!("{base}/{pattern}").into_bytes()]),
            );
        }
        assert_eq!(t.glob("*{5}", GLOB_BRACE), ok(&["wonder12345"]));
        // The iteration with `true` as the pattern: "1" relative to the working directory.
        assert_eq!(php_glob(b"1", 0).unwrap_err().0, GLOB_NOMATCH);
        assert_eq!(
            php_glob(b"1", GLOB_NOCHECK).unwrap().paths,
            vec![b"1".to_vec()]
        );
    }

    #[test]
    fn glob_bracket_negation() {
        // ext/standard/tests/file/glob_bracket_negation.phpt
        let t = TempDir::new("negation");
        for name in ["a", "b", "c", "^", "!"] {
            t.touch(name);
        }
        let cases = [
            ("[!a]", "! ^ b c"),
            ("[^a]", "! ^ b c"),
            ("[!a-b]", "! ^ c"),
            ("[^a-b]", "! ^ c"),
            ("[!]a]", "! ^ b c"),
            ("[^]a]", "! ^ b c"),
            ("[a^]", "^ a"),
        ];
        for (pattern, expected) in cases {
            let got = t.glob(pattern, 0).unwrap().join(" ");
            assert_eq!(got, expected, "{pattern}");
        }
    }

    #[test]
    fn glob_ranges_and_classes() {
        let t = TempDir::new("classes");
        for name in ["a1", "b2", "C3", "d_", "x", "z9"] {
            t.touch(name);
        }
        assert_eq!(t.glob("[a-b]?", 0), ok(&["a1", "b2"]));
        assert_eq!(t.glob("[b-a]?", 0), nomatch());
        assert_eq!(t.glob("[a-cz]?", 0), ok(&["a1", "b2", "z9"]));
        assert_eq!(t.glob("[[:upper:]]*", 0), ok(&["C3"]));
        assert_eq!(t.glob("?[[:digit:]]", 0), ok(&["C3", "a1", "b2", "z9"]));
        assert_eq!(t.glob("[![:lower:]]*", 0), ok(&["C3"]));
        assert_eq!(t.glob("?[[:punct:]]", 0), ok(&["d_"]));
        assert_eq!(t.glob("[[:alpha:][:digit:]]", 0), ok(&["x"]));
        assert_eq!(
            t.glob("[[:xdigit:]][[:digit:]]", 0),
            ok(&["C3", "a1", "b2"])
        );
        let r = php_glob(&t.pattern("[[:digit:]x]"), 0).unwrap();
        assert_eq!(t.rels(&r), ["x"]);
        assert_eq!(r.matchc, 1);
        assert_ne!(r.flags & GLOB_MAGCHAR, 0);
    }

    #[test]
    fn glob_invalid_char_class() {
        let t = TempDir::new("badclass");
        assert_eq!(t.glob("[[:foo:]]", 0), nomatch());
        // With GLOB_NOCHECK the bad class is just characters of the set, followed by ']'.
        assert_eq!(t.glob("[[:foo:]]", GLOB_NOCHECK), ok(&["[[:foo:]]"]));
        t.touch("o]");
        assert_eq!(t.glob("[[:foo:]]", GLOB_NOCHECK), ok(&["o]"]));
        assert_eq!(t.glob("[[:foo:]]", 0), nomatch());
    }

    #[test]
    fn glob_alnum_class_ends_the_pattern() {
        // `[:alnum:]` compiles to class index 0 == EOS: the rest of the pattern is ignored.
        let t = TempDir::new("alnum");
        for name in ["a", "ax", "1", "bx"] {
            t.touch(name);
        }
        assert_eq!(t.glob("[[:alnum:]]x", 0), ok(&["1", "a"]));
        assert_eq!(t.glob("[[:alpha:]]x", 0), ok(&["ax", "bx"]));
    }

    #[test]
    fn glob_unclosed_bracket_is_literal() {
        let t = TempDir::new("bracket");
        t.touch("[ab");
        t.touch("[!x");
        t.touch("[]");
        let r = php_glob(&t.pattern("[ab"), 0).unwrap();
        assert_eq!(t.rels(&r), ["[ab"]);
        assert_eq!(r.flags & GLOB_MAGCHAR, 0);
        assert_eq!(t.glob("[!x", 0), ok(&["[!x"]));
        assert_eq!(t.glob("[]", 0), ok(&["[]"]));
    }

    #[test]
    fn glob_hidden_files() {
        let t = TempDir::new("hidden");
        t.touch(".hidden");
        t.touch("visible");
        t.mkdir(".hdir");
        assert_eq!(t.glob("*", 0), ok(&["visible"]));
        assert_eq!(t.glob("?*", 0), ok(&["visible"]));
        assert_eq!(t.glob("[.v]*", 0), ok(&["visible"]));
        // readdir() returns "." and "..": a leading literal dot matches them.
        let dots = ok(&[".", "..", ".hdir", ".hidden"]);
        assert_eq!(t.glob(".*", 0), dots);
        assert_eq!(t.glob(r"\.*", 0), dots);
        // The `/` is appended before sorting, and '.' < '/'.
        assert_eq!(
            t.glob(".*", GLOB_MARK),
            ok(&["../", "./", ".hdir/", ".hidden"])
        );
        assert_eq!(t.glob(".h*", 0), ok(&[".hdir", ".hidden"]));
        assert_eq!(t.glob(".hdir/..", 0), ok(&[".hdir/.."]));
    }

    #[test]
    fn glob_nonexistent_paths() {
        // ext/standard/tests/file/glob_variation3.phpt
        let t = TempDir::new("missing");
        t.touch("file");
        for pattern in [
            "*.none",
            "?.none",
            "*{hello,world}.none",
            "*/nothere",
            "[aoeu]*.none",
            "directly_not_exists",
            "nodir/*",
            "file/*",
        ] {
            assert_eq!(t.glob(pattern, 0), nomatch(), "{pattern}");
        }
        // globexp2() returns 0 after globbing every alternative, matched or not.
        assert_eq!(t.glob("*{hello,world}.none", GLOB_BRACE), ok(&[]));
    }

    #[test]
    fn glob_escapes() {
        let t = TempDir::new("escape");
        for name in ["a*", "ab", r"a\b", "[ab]"] {
            t.touch(name);
        }
        let r = php_glob(&t.pattern(r"a\*"), 0).unwrap();
        assert_eq!(t.rels(&r), ["a*"]);
        assert_eq!(r.flags & GLOB_MAGCHAR, 0);
        assert_eq!(t.glob(r"a\*", GLOB_NOESCAPE), ok(&[r"a\b"]));
        assert_eq!(t.glob("a*", 0), ok(&["a*", r"a\b", "ab"]));
        assert_eq!(t.glob(r"a\b", 0), ok(&["ab"]));
        assert_eq!(t.glob(r"a\b", GLOB_NOESCAPE), ok(&[r"a\b"]));
        assert_eq!(t.glob(r"\[ab]", 0), ok(&["[ab]"]));
        assert_eq!(t.glob("[ab]", 0), nomatch());
        // NOCHECK returns the pattern without its escapes; a trailing backslash stays.
        assert_eq!(t.glob(r"x\*y", GLOB_NOCHECK), ok(&["x*y"]));
        assert_eq!(
            t.glob(r"x\*y", GLOB_NOCHECK | GLOB_NOESCAPE),
            ok(&[r"x\*y"])
        );
        assert_eq!(t.glob(r"ab\", 0), nomatch());
        assert_eq!(t.glob(r"ab\", GLOB_NOCHECK), ok(&[r"ab\"]));
    }

    #[test]
    fn glob_braces() {
        let t = TempDir::new("brace");
        for name in ["a1", "a2", "b1", "c", "{}", "d/x"] {
            t.touch(name);
        }
        assert_eq!(t.glob("{a,b}1", GLOB_BRACE), ok(&["a1", "b1"]));
        assert_eq!(t.glob("{a,b}1", 0), nomatch());
        // Each alternative is sorted on its own.
        assert_eq!(t.glob("{b,a}*", GLOB_BRACE), ok(&["b1", "a1", "a2"]));
        assert_eq!(t.glob("{a{1,2},c}", GLOB_BRACE), ok(&["a1", "a2", "c"]));
        assert_eq!(t.glob("{a,d/}*", GLOB_BRACE), ok(&["a1", "a2", "d/x"]));
        assert_eq!(t.glob("{[a]1,c}", GLOB_BRACE), ok(&["a1", "c"]));
        assert_eq!(t.glob("{[,]1,c}", GLOB_BRACE), ok(&["c"]));
        assert_eq!(t.glob(r"\{a,b}1", GLOB_BRACE), nomatch());
        // Only a pattern that is exactly "{}" keeps its braces; elsewhere "{}" expands to "".
        assert_eq!(t.glob("{}", 0), ok(&["{}"]));
        let r = php_glob(&t.pattern("{}"), GLOB_BRACE).unwrap();
        assert_eq!(r.paths, [t.pattern("")]);
        let r = php_glob(b"{}", GLOB_BRACE | GLOB_NOCHECK).unwrap();
        assert_eq!(r.paths, [b"{}".to_vec()]);
        let r = php_glob(b"{}x", GLOB_BRACE | GLOB_NOCHECK).unwrap();
        assert_eq!(r.paths, [b"x".to_vec()]);
        assert_eq!(t.glob("{x,y}", GLOB_BRACE | GLOB_NOCHECK), ok(&["x", "y"]));
        // Unbalanced braces glob the pattern up to the brace.
        assert_eq!(t.glob("c{1", GLOB_BRACE), ok(&["c"]));
        assert_eq!(t.glob("c{1", 0), nomatch());
        let r = php_glob(&t.pattern("{a,b}{1,2}"), GLOB_BRACE).unwrap();
        assert_eq!(t.rels(&r), ["a1", "a2", "b1"]);
        assert_eq!(r.matchc, 3);
        assert_eq!(r.flags & GLOB_MAGCHAR, 0);
    }

    #[test]
    fn glob_mark_and_trailing_slash() {
        let t = TempDir::new("mark");
        t.mkdir("sub");
        t.touch("f");
        symlink(t.root.join("sub"), t.root.join("link")).unwrap();
        assert_eq!(t.glob("*/", 0), ok(&["link/", "sub/"]));
        assert_eq!(t.glob("*/", GLOB_MARK), ok(&["link/", "sub/"]));
        assert_eq!(t.glob("sub", GLOB_MARK), ok(&["sub/"]));
        assert_eq!(t.glob("sub", 0), ok(&["sub"]));
        // (Not combined with "*/": lstat("flink/") succeeds on macOS but not on Linux.)
        symlink(t.root.join("f"), t.root.join("flink")).unwrap();
        assert_eq!(t.glob("*", GLOB_MARK), ok(&["f", "flink", "link/", "sub/"]));
    }

    #[test]
    fn glob_nosort_returns_the_same_set() {
        let t = TempDir::new("nosort");
        let names = ["m", "b", "z", "a", "k", "c"];
        for name in names {
            t.touch(name);
        }
        let mut expected: Vec<&str> = names.to_vec();
        expected.sort();
        assert_eq!(t.glob("?", 0), ok(&expected));
        assert_eq!(sorted(t.glob("?", GLOB_NOSORT)), ok(&expected));
    }

    #[test]
    fn glob_nocheck_and_nomagic() {
        let t = TempDir::new("nocheck");
        assert_eq!(t.glob("nothere", GLOB_NOMAGIC), ok(&["nothere"]));
        assert_eq!(t.glob("noth*re", GLOB_NOMAGIC), nomatch());
        assert_eq!(t.glob("noth*re", GLOB_NOCHECK), ok(&["noth*re"]));
        let r = php_glob(&t.pattern("noth?re"), GLOB_NOCHECK).unwrap();
        assert_eq!(r.matchc, 0);
        assert_eq!(r.flags, GLOB_NOCHECK | GLOB_MAGCHAR);
        assert_eq!(php_glob(b"", 0), Err((GLOB_NOMATCH, GlobResult::default())));
        assert_eq!(
            php_glob(b"", GLOB_NOCHECK).unwrap().paths,
            vec![Vec::<u8>::new()]
        );
    }

    #[test]
    fn glob_onlydir_emulation() {
        // The GLOB_ONLYDIR part of glob_variation.phpt.
        let t = TempDir::new("onlydir");
        t.mkdir("wonder1/wonder2");
        t.mkdir("wonder");
        t.touch("wonder12345");
        symlink(t.root.join("wonder"), t.root.join("wonderlink")).unwrap();
        let onlydir = |pattern: &str, flags: i32| {
            let mut r = php_glob(&t.pattern(pattern), flags & GLOB_FLAGMASK).unwrap();
            glob_onlydir_filter(&mut r.paths);
            t.rels(&r)
        };
        assert_eq!(onlydir("wonder?", GLOB_ONLYDIR), ["wonder1"]);
        assert_eq!(
            onlydir("*der*", GLOB_ONLYDIR),
            ["wonder", "wonder1", "wonderlink"]
        );
        assert_eq!(
            onlydir("*der*", GLOB_ONLYDIR | GLOB_MARK),
            ["wonder/", "wonder1/", "wonderlink/"]
        );
        assert_eq!(onlydir("*/*", GLOB_ONLYDIR), ["wonder1/wonder2"]);
        assert!(glob_is_dir(&t.pattern("wonderlink")));
        assert!(!glob_is_dir(&t.pattern("wonder12345")));
        assert!(!glob_is_dir(&t.pattern("nothere")));
    }

    #[test]
    fn glob_multi_level() {
        let t = TempDir::new("multi");
        for name in [
            "dir1/sub1/a.txt",
            "dir1/sub2/b.txt",
            "dir1/sub2/c.log",
            "dir2/subx/d.txt",
            "dir2/sub10/e.txt",
            "other/sub1/f.txt",
        ] {
            t.touch(name);
        }
        let r = php_glob(&t.pattern("dir*/sub?/*.txt"), 0).unwrap();
        assert_eq!(
            t.rels(&r),
            ["dir1/sub1/a.txt", "dir1/sub2/b.txt", "dir2/subx/d.txt"]
        );
        assert_eq!(r.matchc, 3);
        assert_eq!(
            t.glob("*/sub1/*", 0),
            ok(&["dir1/sub1/a.txt", "other/sub1/f.txt"])
        );
        assert_eq!(t.glob("dir1/*/", 0), ok(&["dir1/sub1/", "dir1/sub2/"]));
        assert_eq!(t.glob("dir1//sub1/*", 0), ok(&["dir1//sub1/a.txt"]));
        // Like glob_variation2.phpt's "../glob_test/*" and "./*".
        assert_eq!(
            t.glob("dir1/sub2/../sub1/*", 0),
            ok(&["dir1/sub2/../sub1/a.txt"])
        );
        assert_eq!(t.glob("dir1/./sub1/*", 0), ok(&["dir1/./sub1/a.txt"]));
        assert_eq!(t.glob("dir1/sub1/a.txt", 0), ok(&["dir1/sub1/a.txt"]));
    }

    #[test]
    fn glob_star_matching_is_linear() {
        let t = TempDir::new("linear");
        let a = "a".repeat(100);
        t.touch(&format!("{a}x"));
        t.touch(&format!("{a}y"));
        assert_eq!(t.glob("a*a*a*a*a*a*a*y", 0), ok(&[&format!("{a}y")]));
        assert_eq!(t.glob("*a*a*a*a*a*a*a*z", 0), nomatch());
        assert_eq!(t.glob("a*a?*x", 0), ok(&[&format!("{a}x")]));
    }

    #[test]
    fn glob_errfunc_and_glob_err() {
        let t = TempDir::new("errfunc");
        // php_glob() has no error callback, so GLOB_ERR changes nothing.
        assert_eq!(t.glob("nodir/*", GLOB_ERR), nomatch());

        let mut calls: Vec<(Vec<u8>, i32)> = Vec::new();
        let mut record = |path: &[u8], errno: i32| {
            calls.push((path.to_vec(), errno));
            0
        };
        let mut r = GlobResult::default();
        let code = php_glob_ex(&t.pattern("nodir/*"), 0, Some(&mut record), &mut r);
        assert_eq!(code, GLOB_NOMATCH);
        let code = php_glob_ex(&t.pattern("nodir/*"), GLOB_ERR, Some(&mut record), &mut r);
        assert_eq!(code, GLOB_ABORTED);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, t.pattern("nodir/"));
        let kind = std::io::Error::from_raw_os_error(calls[0].1).kind();
        assert_eq!(kind, ErrorKind::NotFound);

        let mut abort = |_: &[u8], _: i32| 1;
        let code = php_glob_ex(&t.pattern("nodir/*"), 0, Some(&mut abort), &mut r);
        assert_eq!(code, GLOB_ABORTED);
    }

    #[test]
    fn glob_unreadable_directory() {
        let t = TempDir::new("locked");
        t.touch("locked/f");
        t.touch("open/f");
        let locked = t.root.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read anything; the check only means something for other users.
        if fs::read_dir(&locked).is_err() {
            assert_eq!(t.glob("locked/*", 0), nomatch());
            assert_eq!(t.glob("locked/*", GLOB_ERR), nomatch());
            assert_eq!(t.glob("*/*", GLOB_ERR), ok(&["open/f"]));
            let mut errnos = Vec::new();
            let mut record = |_: &[u8], errno: i32| {
                errnos.push(errno);
                0
            };
            let mut r = GlobResult::default();
            let code = php_glob_ex(&t.pattern("*/*"), GLOB_ERR, Some(&mut record), &mut r);
            assert_eq!(code, GLOB_ABORTED);
            let kind = std::io::Error::from_raw_os_error(errnos[0]).kind();
            assert_eq!(kind, ErrorKind::PermissionDenied);
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn glob_append() {
        let t = TempDir::new("append");
        for name in ["b1", "a1", "a2"] {
            t.touch(name);
        }
        let mut r = GlobResult::default();
        assert_eq!(php_glob_ex(&t.pattern("b*"), 0, None, &mut r), 0);
        assert_eq!(php_glob_ex(&t.pattern("a*"), GLOB_APPEND, None, &mut r), 0);
        assert_eq!(t.rels(&r), ["b1", "a1", "a2"]);
        assert_eq!(r.matchc, 2);
        assert_eq!(
            php_glob_ex(&t.pattern("z*"), GLOB_APPEND, None, &mut r),
            GLOB_NOMATCH
        );
        assert_eq!(r.paths.len(), 3);
        assert_eq!(php_glob_ex(&t.pattern("b*"), 0, None, &mut r), 0);
        assert_eq!(t.rels(&r), ["b1"]);
    }

    #[test]
    fn glob_limit_stat() {
        let t = TempDir::new("limitstat");
        for i in 0..GLOB_LIMIT_STAT + 10 {
            t.touch(&format!("f{i}"));
        }
        // "*/" lstat()s every file (and fails): the stat budget runs out first.
        assert_eq!(t.glob("*/", 0), nomatch());
        assert_eq!(t.glob("*/", GLOB_LIMIT), Err((GLOB_NOSPACE, vec![])));
    }

    #[test]
    fn glob_limit_malloc() {
        let t = TempDir::new("limitmalloc");
        let count = 300;
        for i in 0..count {
            t.touch(&format!("{i:0>200}"));
        }
        assert_eq!(t.glob("*", 0).unwrap().len(), count);
        // globextend(): fails once 8 * (2 + pathc) + bytes (with NULs) > 65536.
        let len = t.pattern(&"0".repeat(200)).len() + 1;
        let ptr = size_of::<*const u8>();
        let k = (1..)
            .find(|k| ptr * (k + 1) + k * len > GLOB_LIMIT_MALLOC)
            .unwrap();
        let (code, partial) = t.glob("*", GLOB_LIMIT).unwrap_err();
        assert_eq!(code, GLOB_NOSPACE);
        assert_eq!(partial.len(), k);
    }

    #[test]
    fn glob_path_max() {
        let t = TempDir::new("pathmax");
        let long = format!("{}{}", "x".repeat(PATH_MAX - 1), "*");
        assert_eq!(
            php_glob(long.as_bytes(), GLOB_NOCHECK),
            Err((
                GLOB_NOMATCH,
                GlobResult {
                    flags: GLOB_NOCHECK,
                    ..GlobResult::default()
                }
            ))
        );

        // A directory chain so that `chain/<name>` is PATH_MAX - 3 bytes long: fits, while
        // one more byte of name overflows glob3()'s buffer check (code 1).
        let mut chain = t.root.clone().into_os_string().into_string().unwrap();
        let target = PATH_MAX - 104;
        while chain.len() < target {
            let rem = target - chain.len();
            let k = if rem > 203 { 200 } else { rem - 1 };
            chain.push('/');
            chain.push_str(&"d".repeat(k));
        }
        assert_eq!(chain.len(), target);
        fs::create_dir_all(&chain).unwrap();
        let name = "n".repeat(PATH_MAX - 4 - target);
        fs::write(format!("{chain}/{name}"), b"").unwrap();
        let pattern = format!("{chain}/*");
        let r = php_glob(pattern.as_bytes(), 0).unwrap();
        assert_eq!(r.paths, [format!("{chain}/{name}").into_bytes()]);
        fs::write(format!("{chain}/{name}m"), b"").unwrap();
        assert_eq!(php_glob(pattern.as_bytes(), 0).unwrap_err().0, 1);
        // The check runs before matching, so even a pattern that skips the name fails.
        assert_eq!(
            php_glob(format!("{chain}/q*").as_bytes(), 0).unwrap_err().0,
            1
        );
    }

    #[test]
    fn glob_pattern_is_a_c_string() {
        let t = TempDir::new("nul");
        t.touch("f");
        let mut pattern = t.pattern("f");
        pattern.extend_from_slice(b"\0junk");
        assert_eq!(php_glob(&pattern, 0).unwrap().paths, [t.pattern("f")]);
    }

    fn chars(s: &str) -> Vec<Char> {
        let mut v: Vec<Char> = s.bytes().map(Char::from).collect();
        v.push(EOS);
        v
    }

    fn unchars(v: &[Char]) -> String {
        v.iter()
            .take_while(|&&c| c != EOS)
            .map(|&c| c as u8 as char)
            .collect()
    }

    #[test]
    fn glob_tilde_expansion() {
        let lookup = |user: &[u8]| match user {
            b"" => Some(b"/home/me".to_vec()),
            b"bob" => Some(b"/u/bob".to_vec()),
            _ => None,
        };
        let tilde = |s: &str, flags| unchars(&globtilde(&chars(s), flags, &lookup));
        assert_eq!(tilde("~/x/*", GLOB_TILDE), "/home/me/x/*");
        assert_eq!(tilde("~", GLOB_TILDE), "/home/me");
        assert_eq!(tilde("~bob/y", GLOB_TILDE), "/u/bob/y");
        assert_eq!(tilde("~bob", GLOB_TILDE), "/u/bob");
        assert_eq!(tilde("~nobody/z", GLOB_TILDE), "~nobody/z");
        assert_eq!(tilde("~/x", 0), "~/x");
        assert_eq!(tilde("a~/x", GLOB_TILDE), "a~/x");
        let mut quoted = chars("~/x");
        quoted[0] |= M_PROTECT;
        assert!(matches!(
            globtilde(&quoted, GLOB_TILDE, &lookup),
            Cow::Borrowed(_)
        ));

        // The expansion is cut at PATH_MAX - 1 characters.
        let rest = chars("~/rest");
        let big = vec![b'h'; PATH_MAX + 10];
        let out = globtilde(&rest, GLOB_TILDE, &|_| Some(big.clone()));
        assert_eq!(out.len(), PATH_MAX);
        assert_eq!(out[PATH_MAX - 1], EOS);
        let near = vec![b'h'; PATH_MAX - 4];
        let out = globtilde(&rest, GLOB_TILDE, &|_| Some(near.clone()));
        assert_eq!(unchars(&out), format!("{}/re", "h".repeat(PATH_MAX - 4)));
    }

    #[test]
    fn glob_passwd_lookup() {
        let passwd = b"##\n# comment\nroot:*:0:0:System Administrator:/var/root:/bin/sh\n\
                       bob:x:501:20:Bob:/Users/bob:/bin/zsh\nshort:x:1\n";
        assert_eq!(passwd_home(passwd, b"root"), Some(b"/var/root".to_vec()));
        assert_eq!(passwd_home(passwd, b"bob"), Some(b"/Users/bob".to_vec()));
        assert_eq!(passwd_home(passwd, b"short"), None);
        assert_eq!(passwd_home(passwd, b"alice"), None);
        assert_eq!(passwd_home(passwd, b"ro"), None);
    }

    #[test]
    fn glob_tilde_uses_home() {
        let Some(home) = std::env::var_os("HOME").map(os_bytes) else {
            return;
        };
        let plain = !home.is_empty() && !home.iter().any(|b| b"*?[{\\".contains(b));
        if plain && fs::symlink_metadata(os_path(&home)).is_ok() {
            assert_eq!(php_glob(b"~", GLOB_TILDE).unwrap().paths, [home]);
        }
        assert_eq!(php_glob(b"~", GLOB_NOCHECK).unwrap().paths, [b"~".to_vec()]);
    }
}
