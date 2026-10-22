//! ext/pcre
//!
//! The `preg_*` functions on top of the `php-regex` engine (a PCRE2 10.44
//! port). The module mirrors `ext/pcre/php_pcre.c`:
//!
//! * [`cache`] — `pcre_get_compiled_regex_cache_ex()` (4096-entry cache),
//!   `pcre_handle_exec_error()` and the per-VM state (`preg_last_error()`);
//! * [`matching`] — `preg_match()` / `preg_match_all()` and
//!   `populate_subpat_array()`;
//! * [`replace`] — `preg_replace()`, `preg_filter()`,
//!   `preg_replace_callback()` and `preg_replace_callback_array()`;
//! * [`split`] — `preg_split()` and `preg_grep()`.

mod cache;
mod matching;
mod replace;
mod split;

#[cfg(test)]
mod tests;

use php_regex::PregError;
use php_runtime::ini::{IniDef, modifiable};
use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use cache::{error_code, get_compiled_regex};

pub const PREG_PATTERN_ORDER: i64 = 1;
pub const PREG_SET_ORDER: i64 = 2;
pub const PREG_OFFSET_CAPTURE: i64 = 1 << 8;
pub const PREG_UNMATCHED_AS_NULL: i64 = 1 << 9;

pub const PREG_SPLIT_NO_EMPTY: i64 = 1 << 0;
pub const PREG_SPLIT_DELIM_CAPTURE: i64 = 1 << 1;
pub const PREG_SPLIT_OFFSET_CAPTURE: i64 = 1 << 2;

pub const PREG_GREP_INVERT: i64 = 1 << 0;

/// `php_pcre_version`: the bundled PCRE2's `PCRE2_CONFIG_VERSION`
/// (`PCRE2_MAJOR.PCRE2_MINOR PCRE2_DATE`).
pub const PCRE_VERSION: &str = "10.44 2024-06-07";
pub const PCRE_VERSION_MAJOR: i64 = 10;
pub const PCRE_VERSION_MINOR: i64 = 44;

/// `php_do_pcre_match()`.
fn do_pcre_match(vm: &mut Vm, args: &mut Args, global: bool) -> PResult<Value> {
    let regex = args.str(0).clone();
    let subject = args.str(1).clone();
    // `$matches` is only filled in when the caller passed it.
    let subpats = args.is_passed(2).then(|| args.by_ref(2));
    let flags = args.int(3);
    let start_offset = args.int(4);

    // Compile regex or get it from cache.
    let Some(pce) = get_compiled_regex(vm, &regex)? else {
        return Ok(Value::Bool(false));
    };

    if start_offset == i64::MIN {
        return Err(vm.argument_value_error(5, &format!("must be greater than {}", i64::MIN)));
    }

    matching::match_impl(
        vm,
        &pce,
        &subject,
        subpats.as_ref(),
        global,
        flags,
        start_offset,
    )
}

/// Perform a Perl-style regular expression match.
fn preg_match(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_pcre_match(vm, args, false)
}

/// Perform a Perl-style global regular expression match.
fn preg_match_all(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_pcre_match(vm, args, true)
}

/// `preg_replace_common()`.
fn preg_replace_common(args: &mut Args, vm: &mut Vm, is_filter: bool) -> PResult<Value> {
    let regex = args.get(0).clone();
    let replace = args.get(1).clone();
    let subject = args.get(2).clone();
    let limit = args.int(3);
    let zcount = args.by_ref(4);
    replace::replace_common(
        vm,
        &regex,
        &replace,
        &subject,
        limit,
        Some(&zcount),
        is_filter,
    )
}

/// Perform Perl-style regular expression replacement.
fn preg_replace(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    preg_replace_common(args, vm, false)
}

/// Perform Perl-style regular expression replacement and only return matches.
fn preg_filter(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    preg_replace_common(args, vm, true)
}

/// Perform Perl-style regular expression replacement using replacement callback.
fn preg_replace_callback(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let regex = args.get(0).clone();
    let callback = args.get(1).clone();
    let subject = args.get(2).clone();
    let limit = args.int(3);
    let zcount = args.by_ref(4);
    let flags = args.int(5);
    let (result, replace_count) =
        replace::replace_func_common(vm, &regex, &callback, &subject, limit, flags)?;
    zcount.set(Value::Int(replace_count as i64));
    Ok(result)
}

/// Perform Perl-style regular expression replacement using replacement callbacks.
fn preg_replace_callback_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let pattern = args.array(0).clone();
    let subject = args.get(1).clone();
    let limit = args.int(2);
    let zcount = args.by_ref(3);
    let flags = args.int(4);
    let (result, replace_count) =
        replace::replace_callback_array(vm, &pattern, &subject, limit, flags)?;
    if let Some(n) = replace_count {
        zcount.set(Value::Int(n as i64));
    }
    Ok(result)
}

/// Split string into an array using a perl-style regular expression as a delimiter.
fn preg_split(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let regex = args.str(0).clone();
    let subject = args.str(1).clone();
    let limit = args.int(2);
    let flags = args.int(3);
    let Some(pce) = get_compiled_regex(vm, &regex)? else {
        return Ok(Value::Bool(false));
    };
    Ok(split::split_impl(vm, &pce, &subject, limit, flags))
}

/// Quote regular expression characters plus an optional character.
fn preg_quote(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0);
    // Nothing to do if we got an empty string.
    if s.is_empty() {
        return Ok(Value::empty_string());
    }
    // An empty delimiter reads as NUL, which is quoted as `\000` anyway.
    let delim = args.str_opt(1).and_then(|d| d.first().copied());
    Ok(Value::from(php_regex::preg_quote(s, delim)))
}

/// Searches array and returns entries which match regex.
fn preg_grep(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let regex = args.str(0).clone();
    let input = args.array(1).clone();
    let flags = args.int(2);
    let Some(pce) = get_compiled_regex(vm, &regex)? else {
        return Ok(Value::Bool(false));
    };
    split::grep_impl(vm, &pce, &input, flags)
}

/// Returns the error code of the last regexp execution.
fn preg_last_error(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(error_code(vm) as i64))
}

/// Returns the error message of the last regexp execution.
fn preg_last_error_msg(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::str(error_code(vm).message()))
}

pub static PREG_MATCH: NativeFunction = NativeFunction {
    sig: "preg_match(string $pattern, string $subject, &$matches = null, int $flags = 0, int $offset = 0): int|false",
    handler: preg_match,
};
pub static PREG_MATCH_ALL: NativeFunction = NativeFunction {
    sig: "preg_match_all(string $pattern, string $subject, &$matches = null, int $flags = 0, int $offset = 0): int|false",
    handler: preg_match_all,
};
pub static PREG_REPLACE: NativeFunction = NativeFunction {
    sig: "preg_replace(string|array $pattern, string|array $replacement, string|array $subject, int $limit = -1, &$count = null): string|array|null",
    handler: preg_replace,
};
pub static PREG_FILTER: NativeFunction = NativeFunction {
    sig: "preg_filter(string|array $pattern, string|array $replacement, string|array $subject, int $limit = -1, &$count = null): string|array|null",
    handler: preg_filter,
};
pub static PREG_REPLACE_CALLBACK: NativeFunction = NativeFunction {
    sig: "preg_replace_callback(string|array $pattern, callable $callback, string|array $subject, int $limit = -1, &$count = null, int $flags = 0): string|array|null",
    handler: preg_replace_callback,
};
pub static PREG_REPLACE_CALLBACK_ARRAY: NativeFunction = NativeFunction {
    sig: "preg_replace_callback_array(array $pattern, string|array $subject, int $limit = -1, &$count = null, int $flags = 0): string|array|null",
    handler: preg_replace_callback_array,
};
pub static PREG_SPLIT: NativeFunction = NativeFunction {
    sig: "preg_split(string $pattern, string $subject, int $limit = -1, int $flags = 0): array|false",
    handler: preg_split,
};
pub static PREG_QUOTE: NativeFunction = NativeFunction {
    sig: "preg_quote(string $str, ?string $delimiter = null): string",
    handler: preg_quote,
};
pub static PREG_GREP: NativeFunction = NativeFunction {
    sig: "preg_grep(string $pattern, array $array, int $flags = 0): array|false",
    handler: preg_grep,
};
pub static PREG_LAST_ERROR: NativeFunction = NativeFunction {
    sig: "preg_last_error(): int",
    handler: preg_last_error,
};
pub static PREG_LAST_ERROR_MSG: NativeFunction = NativeFunction {
    sig: "preg_last_error_msg(): string",
    handler: preg_last_error_msg,
};

pub fn register(r: &mut Registry) {
    r.ini_entries(&[
        IniDef::new("pcre.backtrack_limit", Some("1000000"), modifiable::ALL),
        IniDef::new("pcre.recursion_limit", Some("100000"), modifiable::ALL),
        IniDef::new("pcre.jit", Some("1"), modifiable::ALL),
    ]);

    r.functions(&[
        &PREG_MATCH,
        &PREG_MATCH_ALL,
        &PREG_REPLACE,
        &PREG_FILTER,
        &PREG_REPLACE_CALLBACK,
        &PREG_REPLACE_CALLBACK_ARRAY,
        &PREG_SPLIT,
        &PREG_QUOTE,
        &PREG_GREP,
        &PREG_LAST_ERROR,
        &PREG_LAST_ERROR_MSG,
    ]);

    r.constant("PREG_PATTERN_ORDER", PREG_PATTERN_ORDER);
    r.constant("PREG_SET_ORDER", PREG_SET_ORDER);
    r.constant("PREG_OFFSET_CAPTURE", PREG_OFFSET_CAPTURE);
    r.constant("PREG_UNMATCHED_AS_NULL", PREG_UNMATCHED_AS_NULL);
    r.constant("PREG_SPLIT_NO_EMPTY", PREG_SPLIT_NO_EMPTY);
    r.constant("PREG_SPLIT_DELIM_CAPTURE", PREG_SPLIT_DELIM_CAPTURE);
    r.constant("PREG_SPLIT_OFFSET_CAPTURE", PREG_SPLIT_OFFSET_CAPTURE);
    r.constant("PREG_GREP_INVERT", PREG_GREP_INVERT);
    r.constant("PREG_NO_ERROR", PregError::NoError as i64);
    r.constant("PREG_INTERNAL_ERROR", PregError::Internal as i64);
    r.constant(
        "PREG_BACKTRACK_LIMIT_ERROR",
        PregError::BacktrackLimit as i64,
    );
    r.constant(
        "PREG_RECURSION_LIMIT_ERROR",
        PregError::RecursionLimit as i64,
    );
    r.constant("PREG_BAD_UTF8_ERROR", PregError::BadUtf8 as i64);
    r.constant(
        "PREG_BAD_UTF8_OFFSET_ERROR",
        PregError::BadUtf8Offset as i64,
    );
    r.constant("PREG_JIT_STACKLIMIT_ERROR", PregError::JitStackLimit as i64);
    r.constant("PCRE_VERSION", PCRE_VERSION);
    r.constant("PCRE_VERSION_MAJOR", PCRE_VERSION_MAJOR);
    r.constant("PCRE_VERSION_MINOR", PCRE_VERSION_MINOR);
    // The engine has no JIT.
    r.constant("PCRE_JIT_SUPPORT", false);
}
