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
