//! `preg_split()` (`php_pcre_split_impl()`) and `preg_grep()`
//! (`php_pcre_grep_impl()`).

use php_regex::PregError;
use php_runtime::{PResult, PhpArray, PhpStr, Value, Vm};

use super::cache::{Exec, Pce, error_code, handle_exec_error, set_error};
use super::matching::offset_pair_value;
use super::{
    PREG_GREP_INVERT, PREG_SPLIT_DELIM_CAPTURE, PREG_SPLIT_NO_EMPTY, PREG_SPLIT_OFFSET_CAPTURE,
};

/// A piece of the subject, plain or as a `[piece, offset]` pair. An unset
/// range (a delimiter group that did not participate) is `""` / `["", -1]`.
fn piece_value(subject: &[u8], range: Option<(usize, usize)>, offset_capture: bool) -> Value {
    if offset_capture {
        offset_pair_value(subject, range, false)
    } else {
        match range {
            Some((s, e)) => Value::bytes(subject.get(s..e).unwrap_or_default()),
            None => Value::empty_string(),
        }
    }
}

/// `php_pcre_split_impl()`.
pub fn split_impl(
    vm: &mut Vm,
    pce: &Pce,
    subject_str: &PhpStr,
    mut limit_val: i64,
    flags: i64,
) -> Value {
    let subject = subject_str.as_bytes();
    let subject_len = subject.len();
    let no_empty = flags & PREG_SPLIT_NO_EMPTY != 0;
    let delim_capture = flags & PREG_SPLIT_DELIM_CAPTURE != 0;
    let offset_capture = flags & PREG_SPLIT_OFFSET_CAPTURE != 0;

    let mut out = PhpArray::new();

    // Start at the beginning of the string.
    let mut start_offset = 0usize;
    let mut last_match_offset = 0usize;
    set_error(vm, PregError::NoError);

    let skip_matching = if limit_val == -1 {
        false
    } else if limit_val == 0 {
        limit_val = -1;
        false
    } else {
        limit_val <= 1
    };

    if !skip_matching {
        let ex = Exec::new(vm, pce, subject);
        let mut res = ex.first(pce.utf, start_offset);
        'exec: loop {
            match res {
                Ok(Some(mut caps)) => {
                    // `matched:` (re-entered after an empty match).
                    loop {
                        if caps.end() < caps.start() {
                            set_error(vm, PregError::Internal);
                            break 'exec;
                        }

                        if !no_empty || caps.start() != last_match_offset {
                            let range = Some((last_match_offset, caps.start()));
                            let _ = out.append(piece_value(subject, range, offset_capture));
                            // One less left to do
                            if limit_val != -1 {
                                limit_val -= 1;
                            }
                        }

                        if delim_capture {
                            for i in 1..caps.count() {
                                // If we have matched a delimiter
                                let range = caps.get(i);
                                let empty = range.is_none_or(|(s, e)| s == e);
                                if !no_empty || !empty {
                                    let _ = out.append(piece_value(subject, range, offset_capture));
                                }
                            }
                        }

                        // Advance to the position right after the last full match.
                        start_offset = caps.end();
                        last_match_offset = start_offset;

                        // An empty match: retry at the same point with
                        // PCRE2_NOTEMPTY_ATSTART, else advance a character.
                        if start_offset == caps.start() {
                            // Get next piece if no limit or limit not yet reached
                            if limit_val != -1 && limit_val <= 1 {
                                break 'exec;
                            }
                            match ex.retry_empty(start_offset) {
                                Ok(Some(again)) => {
                                    caps = again;
                                    continue;
                                }
                                Ok(None) => {
                                    if start_offset < subject_len {
                                        start_offset += pce.unit_len(subject, start_offset);
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

            // Get next piece if no limit or limit not yet reached and something matched.
            if limit_val != -1 && limit_val <= 1 {
                break;
            }
            res = ex.next(start_offset);
        }

        if error_code(vm) != PregError::NoError {
            return Value::Bool(false);
        }
    }

    // The offset might have been incremented, but without further successful matches.
    let start_offset = last_match_offset;
    if !no_empty || start_offset < subject_len {
        if offset_capture {
            // Add the last (match, offset) pair to the return value.
            let range = Some((start_offset, subject_len));
            let _ = out.append(offset_pair_value(subject, range, false));
        } else if start_offset == 0 {
            let _ = out.append(Value::String(subject_str.clone()));
        } else {
            let _ = out.append(Value::bytes(&subject[start_offset..]));
        }
    }
    Value::array(out)
}

/// `php_pcre_grep_impl()`.
pub fn grep_impl(vm: &mut Vm, pce: &Pce, input: &PhpArray, flags: i64) -> PResult<Value> {
    let invert = flags & PREG_GREP_INVERT != 0;
    let mut out = PhpArray::new();
    set_error(vm, PregError::NoError);

    let base = super::cache::match_options(vm);

    // Go through the input array.
    for (key, entry) in input.iter() {
        let subject_str = vm.to_php_string(entry)?;
        let ex = Exec {
            regex: &pce.regex,
            subject: subject_str.as_bytes(),
            base,
        };

        // Perform the match.
        match ex.first(pce.utf, 0) {
            // If the entry fits our requirements
            Ok(Some(_)) => {
                if !invert {
                    out.insert(key, entry.clone());
                }
            }
            Ok(None) => {
                if invert {
                    out.insert(key, entry.clone());
                }
            }
            Err(e) => {
                handle_exec_error(vm, &e);
                break;
            }
        }
    }

    if error_code(vm) != PregError::NoError {
        return Ok(Value::Bool(false));
    }
    Ok(Value::array(out))
}
