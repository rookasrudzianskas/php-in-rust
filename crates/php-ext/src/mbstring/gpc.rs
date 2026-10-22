//! `ext/mbstring/mb_gpc.c`: `_php_mb_encoding_handler_ex` (query string
//! parsing with encoding detection and conversion) for `mb_parse_str`, and
//! `php_register_variable_ex` from `main/php_variables.c`.

use std::rc::Rc;

use php_runtime::errors::DisplayErrors;
use php_runtime::{ArrayKey, Args, PResult, PhpArray, PhpStr, Value, Vm};

use super::conversion::guess_encoding_for_strings;
use super::globals::{globals, strict_detection};
use super::mbfl::convert::mb_fast_convert;
use super::mbfl::encoding::{Encoding, NoEncoding, enc};
use crate::standard::url::php_url_decode;

/// C-locale `isspace()`.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `php_is_forbidden_variable_name`: mangling must not create a `__Host-`
/// or `__Secure-` prefix that the raw name did not have.
fn is_forbidden(mangled: &[u8], pre_mangled: &[u8]) -> bool {
    [&b"__Host-"[..], &b"__Secure-"[..]]
        .iter()
        .any(|prefix| mangled.starts_with(prefix) && !pre_mangled.starts_with(prefix))
}

/// One operation of `php_register_variable_ex` on the current array.
#[derive(Debug, PartialEq)]
enum Step {
    /// Descend into the array at this key (`None`: a new appended array).
    Container(Option<Vec<u8>>),
    /// Store the value at this key (`None`: append).
    Leaf(Option<Vec<u8>>),
    /// Too deeply nested: drop the whole top-level variable.
    NestingExceeded,
    /// Forbidden name: stop without storing.
    Stop,
}

/// Parses a variable name like `a.b[x][][y` into its base name and the
/// steps `php_register_variable_ex` performs (mirroring its in-place
/// C-string surgery); `None` for an empty name.
fn plan_variable(var_name: &[u8], max_nesting: i64) -> Option<(Vec<u8>, Vec<Step>)> {
    let var_name = &var_name[..var_name.iter().position(|&c| c == 0).unwrap_or(var_name.len())];
    // Ignore leading spaces.
    let pre = &var_name[var_name.iter().position(|&c| c != b' ').unwrap_or(var_name.len())..];
    let mut buf = pre.to_vec();
    buf.push(0);
    let cstrlen = |buf: &[u8], from: usize| buf[from..].iter().position(|&c| c == 0).unwrap_or(0);

    // Spaces and dots become underscores up to the first `[`.
    let mut p = 0;
    let mut is_array = false;
    while buf[p] != 0 {
        match buf[p] {
            b' ' | b'.' => buf[p] = b'_',
            b'[' => {
                is_array = true;
                buf[p] = 0;
                break;
            }
            _ => {}
        }
        p += 1;
    }
    let var_len = p;
    if var_len == 0 {
        return None;
    }
    let base = buf[..var_len].to_vec();

    let key = |buf: &[u8], idx: Option<(usize, usize)>| idx.map(|(s, l)| buf[s..s + l].to_vec());
    let checked = |k: Option<Vec<u8>>, f: fn(Option<Vec<u8>>) -> Step| match k {
        Some(k) if is_forbidden(&k, pre) => Step::Stop,
        k => f(k),
    };

    let mut steps = Vec::new();
    let mut index = Some((0usize, var_len));
    if !is_array {
        steps.push(checked(key(&buf, index), Step::Leaf));
        return Some((base, steps));
    }
    let mut ip = p;
    let mut nest_level = 0i64;
    loop {
        nest_level += 1;
        if nest_level > max_nesting {
            steps.push(Step::NestingExceeded);
            return Some((base, steps));
        }
        ip += 1;
        let index_s = ip;
        if is_space(buf[ip]) {
            ip += 1;
        }
        let new_index = if buf[ip] == b']' {
            None
        } else {
            let end = ip + cstrlen(&buf, ip);
            match buf[ip..end].iter().position(|&c| c == b']') {
                Some(off) => {
                    ip += off;
                    buf[ip] = 0;
                    Some((index_s, ip - index_s))
                }
                None => {
                    // Not an index: un-terminate the name and mangle the rest.
                    buf[index_s - 1] = b'_';
                    let mut q = index_s;
                    while buf[q] != 0 {
                        if matches!(buf[q], b' ' | b'.' | b'[') {
                            buf[q] = b'_';
                        }
                        q += 1;
                    }
                    let idx = index.map(|(s, _)| (s, cstrlen(&buf, s)));
                    steps.push(checked(key(&buf, idx), Step::Leaf));
                    return Some((base, steps));
                }
            }
        };
        let step = checked(key(&buf, index), Step::Container);
        let stop = step == Step::Stop;
        steps.push(step);
        if stop {
            return Some((base, steps));
        }
        index = new_index;
        ip += 1;
        if buf[ip] == b'[' {
            buf[ip] = 0;
        } else {
            steps.push(checked(key(&buf, index), Step::Leaf));
            return Some((base, steps));
        }
    }
}

/// Executes `steps` on `arr`; `true` when the nesting limit was hit.
fn apply_steps(arr: &mut PhpArray, steps: &[Step], val: Value) -> bool {
    let Some(step) = steps.first() else {
        return false;
    };
    match step {
        Step::Stop => false,
        Step::NestingExceeded => true,
        Step::Leaf(None) => {
            let _ = arr.append(val);
            false
        }
        Step::Leaf(Some(k)) => {
            arr.insert(ArrayKey::from_bytes(k), val);
            false
        }
        Step::Container(k) => {
            let slot = match k {
                None => match arr.append(Value::empty_array()) {
                    Ok(i) => arr.get_mut(&ArrayKey::Int(i)).expect("just appended"),
                    Err(_) => return false,
                },
                Some(k) => arr.entry_mut(ArrayKey::from_bytes(k)),
            };
            if !matches!(slot, Value::Array(_)) {
                *slot = Value::empty_array();
            }
            let Value::Array(inner) = slot else {
                unreachable!()
            };
            apply_steps(Rc::make_mut(inner), &steps[1..], val)
        }
    }
}

/// `php_register_variable_safe` into a plain array.
fn register_variable(vm: &mut Vm, var_name: &[u8], val: Value, track: &mut PhpArray) -> PResult<()> {
    let max_nesting = match vm.ini_get("max_input_nesting_level") {
        Some(_) => vm.ini_int("max_input_nesting_level"),
        None => 64,
    };
    let Some((base, steps)) = plan_variable(var_name, max_nesting) else {
        return Ok(());
    };
    if apply_steps(track, &steps, val) {
        track.remove(&ArrayKey::from_bytes(&base));
        // Not displayed, to avoid information disclosure.
        if vm.errors.display == DisplayErrors::Off {
            vm.warning(format!(
                "Input variable nesting level exceeded {max_nesting}. To increase the limit change max_input_nesting_level in php.ini."
            ))?;
        }
    }
    Ok(())
}

/// `_php_mb_encoding_handler_ex` for `PARSE_STRING`: `None` when nothing was
/// parsed (empty input, too many variables).
fn encoding_handler(
    vm: &mut Vm,
    track: &mut PhpArray,
    res: &[u8],
    separator: &[u8],
    to_encoding: &'static Encoding,
    from_encodings: &[&'static Encoding],
) -> PResult<Option<&'static Encoding>> {
    // `res` is a C string.
    let res = &res[..res.iter().position(|&c| c == 0).unwrap_or(res.len())];
    if res.is_empty() {
        return Ok(None);
    }
    // Split (php_strtok_r skips empty tokens) and decode.
    let mut list: Vec<Vec<u8>> = Vec::new();
    for var in res.split(|c| separator.contains(c)).filter(|t| !t.is_empty()) {
        match var.iter().position(|&c| c == b'=') {
            Some(eq) => {
                list.push(php_url_decode(&var[..eq]));
                // The value is a C string after the split.
                list.push(php_url_decode(&var[eq + 1..]));
            }
            None => {
                list.push(php_url_decode(var));
                list.push(Vec::new());
            }
        }
    }

    let max_input_vars = match vm.ini_get("max_input_vars") {
        Some(_) => vm.ini_int("max_input_vars"),
        None => 1000,
    };
    if list.len() as i64 > max_input_vars.saturating_mul(2) {
        vm.warning(format!(
            "Input variables exceeded {max_input_vars}. To increase the limit change max_input_vars in php.ini."
        ))?;
        return Ok(None);
    }

    let pass = enc(NoEncoding::Pass);
    let from_encoding = match from_encodings.len() {
        0 => pass,
        1 => from_encodings[0],
        _ => {
            let strings: Vec<&[u8]> = list.iter().map(|v| v.as_slice()).collect();
            let strict = strict_detection(vm);
            match guess_encoding_for_strings(&strings, from_encodings, strict, false) {
                Some(e) => e,
                None => {
                    vm.warning("Unable to detect encoding")?;
                    pass
                }
            }
        }
    };

    let (substchar, mode) = {
        let g = globals(vm)?;
        (g.current_filter_illegal_substchar, g.current_filter_illegal_mode)
    };
    for pair in list.chunks(2) {
        let (var, val) = (&pair[0], &pair[1]);
        if !from_encoding.is(NoEncoding::Pass) && !to_encoding.is(NoEncoding::Pass) {
            let (cvar, e1) = mb_fast_convert(var, from_encoding, to_encoding, substchar, mode);
            let (cval, e2) = mb_fast_convert(val, from_encoding, to_encoding, substchar, mode);
            globals(vm)?.illegalchars += e1 as i64 + e2 as i64;
            register_variable(vm, &cvar, Value::String(PhpStr::from_vec(cval)), track)?;
        } else {
            register_variable(vm, var, Value::String(PhpStr::from_bytes(val)), track)?;
        }
    }
    Ok(Some(from_encoding))
}

pub fn mb_parse_str(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let result = args.by_ref(1);
    let separator = vm
        .ini_get("arg_separator.input")
        .map(|s| s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())].to_vec())
        .unwrap_or_else(|| b"&".to_vec());
    let (to_encoding, from_encodings) = {
        let g = globals(vm)?;
        (g.current_internal_encoding, g.http_input_list.clone())
    };
    let mut track = PhpArray::new();
    let detected = encoding_handler(vm, &mut track, &s, &separator, to_encoding, &from_encodings)?;
    result.set(Value::array(track));
    globals(vm)?.http_input_identify = detected;
    Ok(Value::Bool(detected.is_some()))
}
