//! `preg_match()` / `preg_match_all()` (`php_pcre_match_impl()`) and the
//! helpers that turn a match into PHP arrays (`populate_subpat_array()`).

use php_regex::{Captures, PregError};
use php_runtime::{ArrayKey, PResult, PhpArray, PhpRef, PhpStr, Value, Vm};

use super::cache::{
    Exec, Pce, error_code, handle_exec_error, is_known_valid_utf8, remember_valid_utf8, set_error,
};
use super::{PREG_OFFSET_CAPTURE, PREG_PATTERN_ORDER, PREG_SET_ORDER, PREG_UNMATCHED_AS_NULL};

/// The text of a `(*MARK)`: PCRE2 hands PHP a C string.
fn mark_value(mark: &[u8]) -> Value {
    let end = mark.iter().position(|&b| b == 0).unwrap_or(mark.len());
    Value::bytes(&mark[..end])
}

/// `populate_match_value()`: the matched text, or `""` / `null` for an unset group.
fn match_value(subject: &[u8], range: Option<(usize, usize)>, unmatched_as_null: bool) -> Value {
    match range {
        Some((s, e)) => Value::bytes(subject.get(s..e).unwrap_or_default()),
        None if unmatched_as_null => Value::Null,
        None => Value::empty_string(),
    }
}

/// `zend_new_pair()`.
fn pair(first: Value, second: i64) -> Value {
    Value::array(PhpArray::from_values(vec![first, Value::Int(second)]))
}

/// The `[match, offset]` pair of `add_offset_pair()`.
pub fn offset_pair_value(
    subject: &[u8],
    range: Option<(usize, usize)>,
    unmatched_as_null: bool,
) -> Value {
    match range {
        Some((s, e)) => pair(match_value(subject, Some((s, e)), false), s as i64),
        None => pair(match_value(subject, None, unmatched_as_null), -1),
    }
}

/// `add_named()`: with `J` several groups can share a name; the one that
/// actually matched wins.
fn add_named(subpats: &mut PhpArray, name: &PhpStr, val: &Value, unmatched: bool) {
    let key = ArrayKey::Str(name.clone());
    // A matched group overwrites (`zend_hash_update`), an unset one only
    // fills a missing key (`zend_hash_add`).
    if !unmatched || !subpats.contains_key(&key) {
        subpats.insert(key, val.clone());
    }
}

/// `add_offset_pair()`: appends an offset pair (also under `name`, if any).
pub fn add_offset_pair(
    result: &mut PhpArray,
    subject: &[u8],
    range: Option<(usize, usize)>,
    name: Option<&PhpStr>,
    unmatched_as_null: bool,
) {
    let match_pair = offset_pair_value(subject, range, unmatched_as_null);
    if let Some(name) = name {
        add_named(result, name, &match_pair, range.is_none());
    }
    let _ = result.append(match_pair);
}

/// `populate_subpat_array()`: fills `subpats` for one match. Trailing unset
/// groups (those at or beyond `caps.count()`) are only added with
/// `PREG_UNMATCHED_AS_NULL`.
pub fn populate_subpat_array(
    subpats: &mut PhpArray,
    subject: &[u8],
    caps: &Captures,
    names: Option<&[Option<PhpStr>]>,
    num_subpats: usize,
    flags: i64,
) {
    let offset_capture = flags & PREG_OFFSET_CAPTURE != 0;
    let unmatched_as_null = flags & PREG_UNMATCHED_AS_NULL != 0;
    let count = caps.count();
    let name_of = |i: usize| names.and_then(|n| n.get(i)).and_then(Option::as_ref);

    if offset_capture {
        for i in 0..count {
            add_offset_pair(subpats, subject, caps.get(i), name_of(i), unmatched_as_null);
        }
        if unmatched_as_null {
            for i in count..num_subpats {
                add_offset_pair(subpats, subject, None, name_of(i), true);
            }
        }
    } else {
        for i in 0..count {
            let range = caps.get(i);
            let val = match_value(subject, range, unmatched_as_null);
            if let Some(name) = name_of(i) {
                add_named(subpats, name, &val, range.is_none());
            }
            let _ = subpats.append(val);
        }
        if unmatched_as_null {
            for i in count..num_subpats {
                if let Some(name) = name_of(i) {
                    let key = ArrayKey::Str(name.clone());
                    if !subpats.contains_key(&key) {
                        subpats.insert(key, Value::Null);
                    }
                }
                let _ = subpats.append(Value::Null);
            }
        }
    }
    // Add MARK, if available.
    if let Some(mark) = caps.mark() {
        subpats.insert(ArrayKey::Str(PhpStr::from("MARK")), mark_value(mark));
    }
}

/// `php_pcre_match_impl()`. Returns the number of matches, or `false` on
/// error (see `preg_last_error()`).
pub fn match_impl(
    vm: &mut Vm,
    pce: &Pce,
    subject_str: &PhpStr,
    subpats: Option<&PhpRef>,
    global: bool,
    flags: i64,
    start_offset: i64,
) -> PResult<Value> {
    let subject = subject_str.as_bytes();
    let subject_len = subject.len();

    // Overwrite the passed-in value for subpatterns with an empty array.
    if let Some(r) = subpats {
        r.set(Value::empty_array());
    }

    let mut subpats_order = if global { PREG_PATTERN_ORDER } else { 0 };
    let offset_capture = flags & PREG_OFFSET_CAPTURE != 0;
    let unmatched_as_null = flags & PREG_UNMATCHED_AS_NULL != 0;
    if flags != 0 && flags & 0xff != 0 {
        subpats_order = flags & 0xff;
        if (global && !(PREG_PATTERN_ORDER..=PREG_SET_ORDER).contains(&subpats_order))
            || (!global && subpats_order != 0)
        {
            return Err(vm.argument_value_error(4, "must be a PREG_* constant"));
        }
    }

    // Negative offset counts from the end of the string.
    let mut start_offset2 = if start_offset < 0 {
        let back = start_offset.unsigned_abs();
        if back <= subject_len as u64 {
            subject_len - back as usize
        } else {
            0
        }
    } else {
        start_offset as usize
    };
    if start_offset2 > subject_len {
        // PCRE2_ERROR_BADOFFSET
        set_error(vm, PregError::Internal);
        return Ok(Value::Bool(false));
    }

    let num_subpats = pce.num_subpats();
    // The name table is only used when the caller wants the subpatterns.
    let names = if subpats.is_some() { pce.names() } else { None };

    let mut matched = 0i64;
    set_error(vm, PregError::NoError);

    let mut match_sets: Option<Vec<PhpArray>> =
        (global && subpats.is_some() && subpats_order == PREG_PATTERN_ORDER)
            .then(|| (0..num_subpats).map(|_| PhpArray::new()).collect());
    let mut marks: Option<PhpArray> = None;
    // The `$matches` array being built.
    let mut out = PhpArray::new();

    let ex = Exec::new(vm, pce, subject);
    let orig_start_offset = start_offset2;
    let check_utf = pce.utf && !is_known_valid_utf8(vm, subject_str, orig_start_offset);
    let mut res = ex.first(check_utf, start_offset2);

    'exec: loop {
        match res {
            Ok(Some(mut caps)) => {
                // `matched:` (re-entered after an empty match with NOTEMPTY_ATSTART).
                loop {
                    matched += 1;
                    let count = caps.count();

                    // If subpatterns array has been passed, fill it in with values.
                    if subpats.is_some() {
                        if caps.end() < caps.start() {
                            // (`$matches` keeps what was collected so far.)
                            let warned = vm.warning("Get subpatterns list failed");
                            if let Some(r) = subpats {
                                r.set(Value::array(out));
                            }
                            warned?;
                            return Ok(Value::Bool(false));
                        }

                        if global {
                            if subpats_order == PREG_PATTERN_ORDER {
                                let sets = match_sets.as_mut().expect("pattern order sets");
                                // For each subpattern, insert it into the appropriate array.
                                for (i, set) in sets.iter_mut().enumerate().take(count) {
                                    let range = caps.get(i);
                                    let val = if offset_capture {
                                        offset_pair_value(subject, range, unmatched_as_null)
                                    } else {
                                        match_value(subject, range, unmatched_as_null)
                                    };
                                    let _ = set.append(val);
                                }
                                if let Some(mark) = caps.mark() {
                                    marks
                                        .get_or_insert_with(PhpArray::new)
                                        .insert(ArrayKey::Int(matched - 1), mark_value(mark));
                                }
                                // If the number of captured subpatterns on this run is
                                // less than the total possible number, pad the result
                                // arrays with NULLs or empty strings.
                                for set in sets.iter_mut().skip(count) {
                                    let val = if offset_capture {
                                        offset_pair_value(subject, None, unmatched_as_null)
                                    } else {
                                        match_value(subject, None, unmatched_as_null)
                                    };
                                    let _ = set.append(val);
                                }
                            } else {
                                // PREG_SET_ORDER
                                let mut result_set = PhpArray::with_capacity(count);
                                populate_subpat_array(
                                    &mut result_set,
                                    subject,
                                    &caps,
                                    names,
                                    num_subpats,
                                    flags,
                                );
                                let _ = out.append(Value::array(result_set));
                            }
                        } else {
                            // single pattern matching
                            populate_subpat_array(
                                &mut out,
                                subject,
                                &caps,
                                names,
                                num_subpats,
                                flags,
                            );
                            break 'exec;
                        }
                    }

                    // Advance to the next piece.
                    start_offset2 = caps.end();

                    // If we have matched an empty string, mimic what Perl's /g
                    // options does: retry at the same point with
                    // PCRE2_NOTEMPTY_ATSTART, otherwise advance a character.
                    if start_offset2 == caps.start() {
                        match ex.retry_empty(start_offset2) {
                            Ok(Some(again)) => {
                                if global {
                                    caps = again;
                                    continue;
                                }
                                break 'exec;
                            }
                            Ok(None) => {
                                if start_offset2 < subject_len {
                                    start_offset2 += pce.unit_len(subject, start_offset2);
                                } else {
                                    break 'exec;
                                }
                            }
                            Err(e) => {
                                handle_exec_error(vm, &e);
                                break 'exec;
                            }
                        }
                    }
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                handle_exec_error(vm, &e);
                break;
            }
        }

        if !global {
            break;
        }

        res = ex.next(start_offset2);
    }

    // Add the match sets to the output array.
    if let Some(sets) = match_sets {
        for (i, set) in sets.into_iter().enumerate() {
            let wrapper = Value::array(set);
            if let Some(name) = names.and_then(|n| n[i].as_ref()) {
                out.insert(ArrayKey::Str(name.clone()), wrapper.clone());
            }
            let _ = out.append(wrapper);
        }
        if let Some(marks) = marks {
            out.insert(ArrayKey::Str(PhpStr::from("MARK")), Value::array(marks));
        }
    }
    if let Some(r) = subpats {
        r.set(Value::array(out));
    }

    if error_code(vm) == PregError::NoError {
        // If there was no error and we're in /u mode, remember that the string
        // is valid UTF-8.
        if pce.utf && orig_start_offset == 0 {
            remember_valid_utf8(vm, subject_str);
        }
        Ok(Value::Int(matched))
    } else {
        Ok(Value::Bool(false))
    }
}
