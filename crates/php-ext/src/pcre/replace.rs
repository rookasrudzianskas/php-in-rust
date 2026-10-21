//! `preg_replace()`, `preg_filter()`, `preg_replace_callback()` and
//! `preg_replace_callback_array()` (`php_pcre_replace_impl()`,
//! `php_pcre_replace_func_impl()` and their wrappers in `php_pcre.c`).

use php_regex::{Captures, MatchError, PregError};
use php_runtime::{ArrayKey, PResult, PhpArray, PhpRef, PhpStr, Value, Vm};

use super::cache::{Exec, MatchResult, Pce, get_compiled_regex, handle_exec_error, set_error};
use super::matching::populate_subpat_array;

/// What `php_pcre_replace_impl()`'s `if (count >= 0 && limit > 0) ... else if
/// (count == PCRE2_ERROR_NOMATCH || limit == 0) ... else error` chain decides.
enum Step {
    Match(Captures),
    NotMatched,
    Error(MatchError),
}

fn classify(result: MatchResult, limit: u64) -> Step {
    match result {
        Ok(Some(caps)) if limit > 0 => Step::Match(caps),
        // No match, or the limit is used up (even if the attempt failed).
        Ok(_) => Step::NotMatched,
        Err(_) if limit == 0 => Step::NotMatched,
        Err(e) => Step::Error(e),
    }
}

/// `preg_get_backref()`: parses `$n`, `\n` and `${n}` (one or two digits) at
/// `*pos`, advancing it past the reference.
fn get_backref(s: &[u8], pos: &mut usize) -> Option<usize> {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let mut walk = *pos;
    if at(walk + 1) == 0 {
        return None;
    }
    let mut in_brace = false;
    if at(walk) == b'$' && at(walk + 1) == b'{' {
        in_brace = true;
        walk += 1;
    }
    walk += 1;

    let mut backref;
    if at(walk).is_ascii_digit() {
        backref = (at(walk) - b'0') as usize;
        walk += 1;
    } else {
        return None;
    }
    if at(walk).is_ascii_digit() {
        backref = backref * 10 + (at(walk) - b'0') as usize;
        walk += 1;
    }
    if in_brace {
        if at(walk) != b'}' {
            return None;
        }
        walk += 1;
    }
    *pos = walk;
    Some(backref)
}

/// Expands the references of a replacement string for one match.
fn append_replacement(out: &mut Vec<u8>, replace: &[u8], subject: &[u8], caps: &Captures) {
    let mut walk = 0;
    let mut walk_last = 0u8;
    while walk < replace.len() {
        let c = replace[walk];
        if c == b'\\' || c == b'$' {
            if walk_last == b'\\' {
                // An escaped `\` or `$`: replaces the backslash copied before.
                if let Some(last) = out.last_mut() {
                    *last = c;
                }
                walk += 1;
                walk_last = 0;
                continue;
            }
            if let Some(backref) = get_backref(replace, &mut walk) {
                if let Some((s, e)) = caps.get(backref) {
                    out.extend_from_slice(subject.get(s..e).unwrap_or_default());
                }
                continue;
            }
        }
        out.push(c);
        walk += 1;
        walk_last = c;
    }
}

/// The matching loop shared by `php_pcre_replace_impl()` and
/// `php_pcre_replace_func_impl()`. `emit` appends the replacement for one
/// match. Returns `None` when an error was recorded (`preg_last_error()`).
fn replace_loop<F>(
    vm: &mut Vm,
    pce: &Pce,
    subject: &PhpStr,
    mut limit: u64,
    replace_count: &mut usize,
    refresh_limits: bool,
    mut emit: F,
) -> PResult<Option<PhpStr>>
where
    F: FnMut(&mut Vm, &Captures, &mut Vec<u8>) -> PResult<()>,
{
    let subject_b = subject.as_bytes();
    let subject_len = subject_b.len();
    set_error(vm, PregError::NoError);

    let mut ex = Exec::new(vm, pce, subject_b);
    let mut result: Option<Vec<u8>> = None;
    let mut last_end_offset = 0usize;
    let mut step = classify(ex.first(pce.utf, 0), limit);

    loop {
        match step {
            Step::Match(mut caps) => {
                // `matched:` (re-entered after an empty match).
                step = loop {
                    if caps.end() < caps.start() {
                        set_error(vm, PregError::Internal);
                        return Ok(None);
                    }
                    *replace_count += 1;

                    let out = result.get_or_insert_with(Vec::new);
                    if caps.start() > last_end_offset {
                        // the part of the subject before the match
                        out.extend_from_slice(&subject_b[last_end_offset..caps.start()]);
                    }
                    if let Err(e) = emit(vm, &caps, out) {
                        set_error(vm, PregError::Internal);
                        return Err(e);
                    }
                    if refresh_limits {
                        ex.refresh_limits(vm);
                    }
                    limit -= 1;

                    // Advance to the next piece.
                    let mut start_offset = caps.end();
                    last_end_offset = start_offset;

                    // After an empty match, retry at the same point with
                    // PCRE2_NOTEMPTY_ATSTART; if that fails advance a character.
                    if start_offset == caps.start() {
                        match classify(ex.retry_empty(start_offset), limit) {
                            Step::Match(again) => {
                                caps = again;
                                continue;
                            }
                            Step::NotMatched => {
                                if start_offset < subject_len {
                                    start_offset += pce.unit_len(subject_b, start_offset);
                                } else {
                                    break Step::NotMatched;
                                }
                            }
                            Step::Error(e) => break Step::Error(e),
                        }
                    }
                    break classify(ex.next(start_offset), limit);
                };
            }
            Step::NotMatched => {
                return Ok(Some(match result {
                    // Nothing was replaced: the subject itself is the result.
                    None => subject.clone(),
                    Some(mut out) => {
                        // stick that last bit of string on our output
                        out.extend_from_slice(&subject_b[last_end_offset..]);
                        PhpStr::from_vec(out)
                    }
                }));
            }
            Step::Error(e) => {
                handle_exec_error(vm, &e);
                return Ok(None);
            }
        }
    }
}

/// `php_pcre_replace_impl()`.
pub fn replace_impl(
    vm: &mut Vm,
    pce: &Pce,
    subject: &PhpStr,
    replace: &[u8],
    limit: u64,
    replace_count: &mut usize,
) -> PResult<Option<PhpStr>> {
    let subject_b = subject.as_bytes();
    replace_loop(
        vm,
        pce,
        subject,
        limit,
        replace_count,
        false,
        |_, caps, out| {
            append_replacement(out, replace, subject_b, caps);
            Ok(())
        },
    )
}

/// `php_pcre_replace_func_impl()`: the replacement for each match is the
/// string returned by `callable`, called with the match array.
fn replace_func_impl(
    vm: &mut Vm,
    pce: &Pce,
    subject: &PhpStr,
    callable: &Value,
    limit: u64,
    replace_count: &mut usize,
    flags: i64,
) -> PResult<Option<PhpStr>> {
    let subject_b = subject.as_bytes();
    let num_subpats = pce.num_subpats();
    let names = pce.names();
    replace_loop(
        vm,
        pce,
        subject,
        limit,
        replace_count,
        true,
        |vm, caps, out| {
            // `preg_do_repl_func()`
            let mut arg =
                PhpArray::with_capacity(caps.count() + usize::from(caps.mark().is_some()));
            populate_subpat_array(&mut arg, subject_b, caps, names, num_subpats, flags);
            let ret = vm.call(callable, vec![Value::array(arg)])?.into_unref();
            match ret {
                Value::String(s) => out.extend_from_slice(&s),
                other => {
                    let s = vm.to_php_string(&other)?;
                    out.extend_from_slice(&s);
                }
            }
            Ok(())
        },
    )
}

/// `php_pcre_replace()`: compiles `regex` (or takes it from the cache) and
/// replaces in `subject`. `None` on failure.
fn replace_string(
    vm: &mut Vm,
    regex: &[u8],
    subject: &PhpStr,
    replace: &[u8],
    limit: u64,
    count: &mut usize,
) -> PResult<Option<PhpStr>> {
    let Some(pce) = get_compiled_regex(vm, regex)? else {
        return Ok(None);
    };
    replace_impl(vm, &pce, subject, replace, limit, count)
}

/// `php_pcre_replace_func()`.
fn replace_func(
    vm: &mut Vm,
    regex: &[u8],
    subject: &PhpStr,
    callable: &Value,
    limit: u64,
    count: &mut usize,
    flags: i64,
) -> PResult<Option<PhpStr>> {
    let Some(pce) = get_compiled_regex(vm, regex)? else {
        return Ok(None);
    };
    replace_func_impl(vm, &pce, subject, callable, limit, count, flags)
}

/// `php_pcre_replace_array()`: applies each pattern of `regex` in turn to the
/// result of the previous one.
fn replace_array(
    vm: &mut Vm,
    regex: &PhpArray,
    replace: &Value,
    subject: &PhpStr,
    limit: u64,
    count: &mut usize,
) -> PResult<Option<PhpStr>> {
    let mut subject = subject.clone();
    match replace {
        Value::Array(replace_ht) => {
            let mut replacements = replace_ht.values();
            for regex_entry in regex.values() {
                let regex_str = vm.to_php_string(regex_entry)?;
                // Running out of replacement strings means "".
                let replace_str = match replacements.next() {
                    Some(v) => vm.to_php_string(v)?,
                    None => PhpStr::new(),
                };
                match replace_string(vm, &regex_str, &subject, &replace_str, limit, count)? {
                    Some(r) => subject = r,
                    None => return Ok(None),
                }
            }
        }
        replace => {
            let Value::String(replace_str) = replace else {
                unreachable!("replacement is a string or an array")
            };
            for regex_entry in regex.values() {
                let regex_str = vm.to_php_string(regex_entry)?;
                match replace_string(vm, &regex_str, &subject, replace_str, limit, count)? {
                    Some(r) => subject = r,
                    None => return Ok(None),
                }
            }
        }
    }
    Ok(Some(subject))
}

/// `php_replace_in_subject()`.
fn replace_in_subject(
    vm: &mut Vm,
    regex: &Value,
    replace: &Value,
    subject: &PhpStr,
    limit: u64,
    count: &mut usize,
) -> PResult<Option<PhpStr>> {
    match regex {
        Value::Array(regex_ht) => replace_array(vm, regex_ht, replace, subject, limit, count),
        Value::String(regex_str) => {
            let Value::String(replace_str) = replace else {
                unreachable!("replacement is a string when the pattern is")
            };
            replace_string(vm, regex_str, subject, replace_str, limit, count)
        }
        _ => unreachable!("pattern is a string or an array"),
    }
}

/// `_preg_replace_common()`: `regex`, `replace` and `subject` are each a
/// string or an array. Returns the result, or the new `$count` is written to
/// `zcount`.
pub fn replace_common(
    vm: &mut Vm,
    regex: &Value,
    replace: &Value,
    subject: &Value,
    limit: i64,
    zcount: Option<&PhpRef>,
    is_filter: bool,
) -> PResult<Value> {
    let limit = limit as u64;
    // If replace is an array then the regex argument needs to also be an array.
    if matches!(replace, Value::Array(_)) && !matches!(regex, Value::Array(_)) {
        return Err(vm.argument_type_error(
            1,
            "must be of type array when argument #2 ($replacement) is an array, string given",
        ));
    }

    let mut replace_count = 0usize;
    let result = match subject {
        Value::String(subject_str) => {
            let old_replace_count = replace_count;
            match replace_in_subject(vm, regex, replace, subject_str, limit, &mut replace_count)? {
                Some(r) if !is_filter || replace_count > old_replace_count => Value::String(r),
                _ => Value::Null,
            }
        }
        Value::Array(subject_ht) => {
            // For each subject entry, convert it to string, then perform
            // replacement and add the result to the return value array.
            let mut out = PhpArray::with_capacity(subject_ht.len());
            for (key, entry) in subject_ht.iter() {
                let old_replace_count = replace_count;
                let entry_str = vm.to_php_string(entry)?;
                let result =
                    replace_in_subject(vm, regex, replace, &entry_str, limit, &mut replace_count)?;
                match result {
                    Some(r) if !is_filter || replace_count > old_replace_count => {
                        out.insert(key, Value::String(r));
                    }
                    _ => {}
                }
            }
            Value::array(out)
        }
        _ => unreachable!("subject is a string or an array"),
    };

    if let Some(r) = zcount {
        r.set(Value::Int(replace_count as i64));
    }
    Ok(result)
}

/// `php_replace_in_subject_func()`.
fn replace_in_subject_func(
    vm: &mut Vm,
    regex: &Value,
    callable: &Value,
    subject: &PhpStr,
    limit: u64,
    count: &mut usize,
    flags: i64,
) -> PResult<Option<PhpStr>> {
    match regex {
        Value::String(regex_str) => {
            replace_func(vm, regex_str, subject, callable, limit, count, flags)
        }
        Value::Array(regex_ht) => {
            // If regex is an array
            let mut subject = subject.clone();
            for entry in regex_ht.values() {
                let regex_str = vm.to_php_string(entry)?;
                match replace_func(vm, &regex_str, &subject, callable, limit, count, flags)? {
                    Some(r) => subject = r,
                    None => return Ok(None),
                }
            }
            Ok(Some(subject))
        }
        _ => unreachable!("pattern is a string or an array"),
    }
}

/// `php_preg_replace_func_impl()`: returns the result and the replacement count.
pub fn replace_func_common(
    vm: &mut Vm,
    regex: &Value,
    callable: &Value,
    subject: &Value,
    limit: i64,
    flags: i64,
) -> PResult<(Value, usize)> {
    let limit = limit as u64;
    let mut replace_count = 0usize;
    let result = match subject {
        Value::String(subject_str) => {
            match replace_in_subject_func(
                vm,
                regex,
                callable,
                subject_str,
                limit,
                &mut replace_count,
                flags,
            )? {
                Some(r) => Value::String(r),
                None => Value::Null,
            }
        }
        Value::Array(subject_ht) => {
            let mut out = PhpArray::with_capacity(subject_ht.len());
            for (key, entry) in subject_ht.iter() {
                let entry_str = vm.to_php_string(entry)?;
                let result = replace_in_subject_func(
                    vm,
                    regex,
                    callable,
                    &entry_str,
                    limit,
                    &mut replace_count,
                    flags,
                )?;
                if let Some(r) = result {
                    out.insert(key, Value::String(r));
                }
            }
            Value::array(out)
        }
        _ => unreachable!("subject is a string or an array"),
    };
    Ok((result, replace_count))
}

/// The body of `preg_replace_callback_array()`.
pub fn replace_callback_array(
    vm: &mut Vm,
    pattern: &PhpArray,
    subject: &Value,
    limit: i64,
    flags: i64,
) -> PResult<(Value, Option<usize>)> {
    let mut subject = subject.clone();
    let mut replace_count = 0usize;
    for (key, replace) in pattern.iter() {
        let ArrayKey::Str(regex) = key else {
            return Err(vm.argument_type_error(1, "must contain only string patterns as keys"));
        };
        let callable = replace.unref();
        let host = vm.host.clone();
        if host.check_callable(vm, &callable).is_err() {
            return Err(vm.argument_type_error(1, "must contain only valid callbacks"));
        }
        let (result, n) =
            replace_func_common(vm, &Value::String(regex), &callable, &subject, limit, flags)?;
        replace_count += n;
        match result {
            Value::Null => return Ok((Value::Null, None)),
            other => subject = other,
        }
    }
    Ok((subject, Some(replace_count)))
}
