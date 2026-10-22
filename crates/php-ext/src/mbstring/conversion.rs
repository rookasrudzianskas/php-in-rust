//! Conversion functions of `mbstring.c`: `mb_convert_encoding`,
//! `mb_detect_encoding` (and the encoding guesser), `mb_convert_variables`,
//! `mb_convert_kana` and the numeric entity functions.

use std::rc::Rc;

use php_runtime::{Args, ArrayKey, PResult, PhpArray, PhpStr, Value, Vm};

use super::filters::cjk::iso2022::mb_convert_kana_codepoint;
use super::filters::cjk::kana::*;
use super::globals::{
    all_encodings_list, get_encoding, globals, illegal_settings, parse_encoding_array,
    parse_encoding_list, strict_detection,
};
use super::mbfl::convert::{ConvertBuf, WCHAR_BUFSIZE};
use super::mbfl::encoding::{BAD_INPUT, Encoding, NoEncoding};
use super::strings::{check_encoding, convert_encoding_ex, is_valid_utf8};
use super::tables::rare_cp_bitvec::RARE_CODEPOINT_BITVEC;

/// `remove_non_encodings_from_elist`: drops Base64, QPrint & co.
fn remove_non_encodings(list: &mut Vec<&'static Encoding>) {
    list.retain(|e| e.no > NoEncoding::CharsetMin);
}

/// `estimate_demerits`: rare codepoints and ASCII punctuation make a
/// candidate encoding less likely.
fn estimate_demerits(w: u32) -> u64 {
    if w > 0xFFFF {
        40
    } else if (0x21..=0x2F).contains(&w) {
        6
    } else if (RARE_CODEPOINT_BITVEC[(w >> 5) as usize] >> (w & 0x1F)) & 1 != 0 {
        30
    } else {
        1
    }
}

/// `struct candidate`.
struct Candidate<'a> {
    enc: &'static Encoding,
    input: &'a [u8],
    demerits: u64,
    state: u32,
    multiplier: f32,
}

/// `init_candidate_array`.
fn init_candidates<'a>(
    encodings: &[&'static Encoding],
    strings: &[&'a [u8]],
    strict: bool,
    order_significant: bool,
) -> Vec<Candidate<'a>> {
    let length = encodings.len();
    let mut out = Vec::with_capacity(length);
    'next: for (i, &e) in encodings.iter().enumerate() {
        let mut demerits = 0u64;
        // Specialised validation functions eliminate candidates early.
        if let Some(check) = e.check {
            for s in strings {
                if !check(s) {
                    if strict {
                        continue 'next;
                    }
                    demerits += 500;
                }
            }
        }
        let multiplier = if order_significant {
            (1.0 + ((0.3 * i as f64) / length as f64)) as f32
        } else {
            1.0
        };
        out.push(Candidate {
            enc: e,
            input: &[],
            demerits,
            state: 0,
            multiplier,
        });
    }
    out
}

/// `start_string`: skips a byte order mark for UTF-8 / UTF-16BE / UTF-16LE.
fn start_string<'a>(array: &mut [Candidate<'a>], input: &'a [u8]) {
    for c in array.iter_mut() {
        c.input = input;
        match c.enc.no {
            NoEncoding::Utf8 if input.starts_with(&[0xEF, 0xBB, 0xBF]) => c.input = &input[3..],
            NoEncoding::Utf16be if input.starts_with(&[0xFE, 0xFF]) => c.input = &input[2..],
            NoEncoding::Utf16le if input.starts_with(&[0xFF, 0xFE]) => c.input = &input[2..],
            _ => {}
        }
    }
}

/// `count_demerits`: decodes with every candidate in parallel.
fn count_demerits(array: &mut Vec<Candidate<'_>>, strict: bool) {
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut finished = array.iter().filter(|c| c.input.is_empty()).count();
    while (strict || array.len() > 1) && finished < array.len() {
        // Reverse order, so eliminated candidates don't shift unvisited ones.
        let mut i = array.len();
        while i > 0 {
            i -= 1;
            if array[i].input.is_empty() {
                continue;
            }
            let c = &mut array[i];
            let n = (c.enc.to_wchar)(&mut c.input, &mut wchar_buf, &mut c.state);
            let mut eliminated = false;
            for &w in wchar_buf[..n].iter().rev() {
                if w == BAD_INPUT {
                    if strict {
                        eliminated = true;
                        break;
                    }
                    c.demerits += 1000;
                } else {
                    c.demerits += estimate_demerits(w);
                }
            }
            if eliminated {
                array.remove(i);
                continue;
            }
            if array[i].input.is_empty() {
                finished += 1;
            }
        }
    }
    for c in array.iter_mut() {
        let d = c.demerits as f64 * c.multiplier as f64;
        c.demerits = if d < u64::MAX as f64 {
            d as u64
        } else {
            u64::MAX
        };
    }
}

/// `mb_guess_encoding_for_strings`.
pub fn guess_encoding_for_strings(
    strings: &[&[u8]],
    elist: &[&'static Encoding],
    strict: bool,
    order_significant: bool,
) -> Option<&'static Encoding> {
    if elist.is_empty() {
        return None;
    }
    if elist.len() == 1 {
        if strict && strings.iter().any(|s| !check_encoding(s, elist[0])) {
            return None;
        }
        return Some(elist[0]);
    }
    if strings.len() == 1 && strings[0].is_empty() {
        return Some(elist[0]);
    }
    let mut array = init_candidates(elist, strings, strict, order_significant);
    for s in strings.iter().rev() {
        start_string(&mut array, s);
        count_demerits(&mut array, strict);
        if array.is_empty() {
            return None;
        }
    }
    let mut best = 0;
    for i in 1..array.len() {
        if array[i].demerits < array[best].demerits {
            best = i;
        }
    }
    Some(array[best].enc)
}

/// `mb_guess_encoding`.
pub fn guess_encoding(
    input: &[u8],
    elist: &[&'static Encoding],
    strict: bool,
    order_significant: bool,
) -> Option<&'static Encoding> {
    guess_encoding_for_strings(&[input], elist, strict, order_significant)
}

/// `php_mb_convert_encoding`: `None` when detection fails (after the warning).
fn convert_encoding_detect(
    vm: &mut Vm,
    input: &[u8],
    to: &'static Encoding,
    from: &[&'static Encoding],
) -> PResult<Option<Vec<u8>>> {
    let from = if from.len() == 1 {
        from[0]
    } else {
        let strict = strict_detection(vm);
        match guess_encoding(input, from, strict, true) {
            Some(e) => e,
            None => {
                vm.warning("Unable to detect character encoding")?;
                return Ok(None);
            }
        }
    };
    Ok(Some(convert_encoding_ex(vm, input, to, from)?))
}

/// `php_mb_convert_encoding_recursive`.
fn convert_encoding_recursive(
    vm: &mut Vm,
    input: &PhpArray,
    to: &'static Encoding,
    from: &[&'static Encoding],
    stack: &mut Vec<*const PhpArray>,
) -> PResult<Option<PhpArray>> {
    let ptr = input as *const PhpArray;
    if stack.contains(&ptr) {
        vm.warning("Cannot convert recursively referenced values")?;
        return Ok(None);
    }
    stack.push(ptr);
    let mut output = PhpArray::with_capacity(input.len());
    for (key, entry) in input.iter() {
        let key = match key {
            ArrayKey::Str(k) => match convert_encoding_detect(vm, &k, to, from)? {
                Some(k) => ArrayKey::Str(PhpStr::from_vec(k)),
                None => continue,
            },
            k => k,
        };
        let entry = entry.unref();
        let value = match entry {
            Value::String(s) => match convert_encoding_detect(vm, &s, to, from)? {
                Some(v) => Value::String(PhpStr::from_vec(v)),
                None => continue,
            },
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => entry,
            Value::Array(a) => match convert_encoding_recursive(vm, &a, to, from, stack)? {
                Some(a) => Value::array(a),
                None => Value::empty_array(),
            },
            _ => {
                vm.warning("Object is not supported")?;
                continue;
            }
        };
        // zend_hash_add / zend_hash_index_add: an existing key is kept.
        if !output.contains_key(&key) {
            output.insert(key, value);
        }
    }
    stack.pop();
    Ok(Some(output))
}

pub fn mb_convert_encoding(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let to_name = args.str(1).clone();
    let to = get_encoding(vm, Some(&to_name), 2)?;
    let mut from = match args.get(2).clone() {
        Value::Array(a) => parse_encoding_array(vm, &a, 3)?,
        Value::String(s) => parse_encoding_list(vm, &s, 3)?,
        _ => vec![globals(vm)?.current_internal_encoding],
    };
    if from.len() > 1 {
        remove_non_encodings(&mut from);
    }
    if from.is_empty() {
        return Err(vm.argument_value_error(3, "must specify at least one encoding"));
    }
    match args.get(0).clone() {
        Value::String(s) => Ok(match convert_encoding_detect(vm, &s, to, &from)? {
            Some(v) => Value::String(PhpStr::from_vec(v)),
            None => Value::Bool(false),
        }),
        Value::Array(a) => {
            let out = convert_encoding_recursive(vm, &a, to, &from, &mut Vec::new())?;
            Ok(Value::array(out.unwrap_or_default()))
        }
        _ => unreachable!("ZPP guarantees array|string"),
    }
}

pub fn mb_detect_encoding(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let mut order_significant = true;
    let mut elist = match args.get(1).clone() {
        Value::Array(a) => {
            // The whole list from mb_list_encodings(): order is meaningless.
            if let Some(all) = &globals(vm)?.all_encodings_list
                && Rc::ptr_eq(all, &a)
            {
                order_significant = false;
            }
            parse_encoding_array(vm, &a, 2)?
        }
        Value::String(list) => parse_encoding_list(vm, &list, 2)?,
        _ => globals(vm)?.current_detect_order_list.clone(),
    };
    if elist.is_empty() {
        return Err(vm.argument_value_error(2, "must specify at least one encoding"));
    }
    remove_non_encodings(&mut elist);
    if elist.is_empty() {
        return Ok(Value::Bool(false));
    }
    let strict = if args.is_passed(2) {
        args.bool(2)
    } else {
        strict_detection(vm)
    };
    let ret = if elist.len() == 1 && elist[0].is(NoEncoding::Utf8) && is_valid_utf8(&s) {
        Some(elist[0])
    } else {
        guess_encoding(&s, &elist, strict, order_significant)
    };
    Ok(match ret {
        Some(e) => Value::from(e.name),
        None => Value::Bool(false),
    })
}

/// `jp_kana_convert`.
fn jp_kana_convert(
    input: &[u8],
    encoding: &'static Encoding,
    mode: u32,
    illegal: (u32, u32),
) -> Vec<u8> {
    // Each codepoint may expand to 2 when converting fullwidth to halfwidth.
    let mut wchar_buf = [0u32; 64];
    let mut converted: Vec<u32> = Vec::with_capacity(128);
    let mut buf_offset = 0usize;
    let mut state = 0u32;
    let mut inp = input;
    let mut buf = ConvertBuf::new(input.len(), illegal.0, illegal.1);

    while !inp.is_empty() {
        converted.clear();
        // A codepoint left over from the previous batch stays in wchar_buf[0].
        let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf[buf_offset..], &mut state);
        let out_len = n + buf_offset;
        if out_len == 0 {
            continue;
        }
        let mut emit_now = false;
        let mut i = 0;
        while i + 1 < out_len {
            let mut second = 0u32;
            let mut consumed = false;
            converted.push(mb_convert_kana_codepoint(
                wchar_buf[i],
                wchar_buf[i + 1],
                Some(&mut consumed),
                &mut second,
                mode,
            ));
            if second != 0 {
                converted.push(second);
            }
            if consumed {
                i += 1;
                if i == out_len - 1 {
                    // Both of the last two codepoints were consumed.
                    buf_offset = 0;
                    emit_now = true;
                    break;
                }
            }
            i += 1;
        }
        if !emit_now {
            if inp.is_empty() {
                let mut second = 0u32;
                converted.push(mb_convert_kana_codepoint(
                    wchar_buf[out_len - 1],
                    0,
                    None,
                    &mut second,
                    mode,
                ));
                if second != 0 {
                    converted.push(second);
                }
            } else {
                // Reprocess the last codepoint with the next batch.
                wchar_buf[0] = wchar_buf[out_len - 1];
                buf_offset = 1;
            }
        }
        (encoding.from_wchar)(&converted, &mut buf, inp.is_empty());
    }
    buf.into_bytes()
}

/// `mb_convert_kana_flags`.
const KANA_FLAGS: [u8; 17] = *b"ARNSKHMCarnskhmcV";

pub fn mb_convert_kana(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let optstr = args.str(1).clone();
    let opt = if args.is_passed(1) || !optstr.is_empty() {
        let mut opt = 0u32;
        for &c in optstr.iter() {
            if c == b'A' {
                opt |= HAN2ZEN_ALL | HAN2ZEN_ALPHA | HAN2ZEN_NUMERIC;
            } else if c == b'a' {
                opt |= ZEN2HAN_ALL | ZEN2HAN_ALPHA | ZEN2HAN_NUMERIC;
            } else if let Some(i) = KANA_FLAGS.iter().position(|&f| f == c) {
                opt |= 1 << i;
            } else {
                return Err(
                    vm.argument_value_error(2, &format!("contains invalid flag: '{}'", c as char))
                );
            }
        }
        // Converting the same characters both ways makes no sense.
        let badflag = ((opt & 0xFF00) >> 8) & opt;
        if badflag != 0 {
            let i = badflag.trailing_zeros() as usize;
            let mut flag1 = KANA_FLAGS[i];
            let mut flag2 = KANA_FLAGS[i + 8];
            if (flag1 == b'R' || flag1 == b'N') && opt & HAN2ZEN_ALL != 0 {
                flag1 = b'A';
            }
            if (flag2 == b'r' || flag2 == b'n') && opt & ZEN2HAN_ALL != 0 {
                flag2 = b'a';
            }
            return Err(vm.argument_value_error(
                2,
                &format!(
                    "must not combine '{}' and '{}' flags",
                    flag1 as char, flag2 as char
                ),
            ));
        }
        if opt & HAN2ZEN_HIRAGANA != 0 && opt & HAN2ZEN_KATAKANA != 0 {
            return Err(vm.argument_value_error(2, "must not combine 'H' and 'K' flags"));
        }
        if opt & ZEN2HAN_HIRAGANA != 0 {
            if opt & ZENKAKU_HIRA2KATA != 0 {
                return Err(vm.argument_value_error(2, "must not combine 'h' and 'C' flags"));
            } else if opt & ZENKAKU_KATA2HIRA != 0 {
                return Err(vm.argument_value_error(2, "must not combine 'h' and 'c' flags"));
            }
        } else if opt & ZEN2HAN_KATAKANA != 0 {
            if opt & ZENKAKU_HIRA2KATA != 0 {
                return Err(vm.argument_value_error(2, "must not combine 'k' and 'C' flags"));
            } else if opt & ZENKAKU_KATA2HIRA != 0 {
                return Err(vm.argument_value_error(2, "must not combine 'k' and 'c' flags"));
            }
        }
        opt
    } else {
        HAN2ZEN_KATAKANA | HAN2ZEN_GLUE
    };
    let encoding = get_encoding(vm, args.str_opt(2), 3)?;
    let illegal = illegal_settings(vm)?;
    Ok(Value::String(PhpStr::from_vec(jp_kana_convert(
        &s, encoding, opt, illegal,
    ))))
}

// ----- mb_convert_variables ---------------------------------------------

/// Identity of an array or object for recursion detection.
fn container_id(v: &Value) -> Option<usize> {
    match v {
        Value::Array(a) => Some(Rc::as_ptr(a) as *const () as usize),
        Value::Object(o) => Some(o.id() as usize | (1 << 62)),
        _ => None,
    }
}

/// `mb_recursive_find_strings` (with `mb_recursive_count_strings`): `Err`
/// on a recursive structure.
fn find_strings(v: &Value, out: &mut Vec<PhpStr>, stack: &mut Vec<usize>) -> Result<(), ()> {
    let v = v.unref();
    match &v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(_) | Value::Object(_) => {
            let id = container_id(&v).expect("container");
            if stack.contains(&id) {
                return Err(());
            }
            stack.push(id);
            let entries: Vec<Value> = match &v {
                Value::Array(a) => a.values().cloned().collect(),
                Value::Object(o) => o.properties().into_iter().map(|(_, v)| v).collect(),
                _ => unreachable!(),
            };
            for e in &entries {
                find_strings(e, out, stack)?;
            }
            stack.pop();
        }
        _ => {}
    }
    Ok(())
}

/// `mb_recursive_convert_variable`: converts `v` in place; `Ok(true)` on a
/// recursive structure.
fn convert_variable(
    vm: &mut Vm,
    arg_num: usize,
    v: &mut Value,
    from: &'static Encoding,
    to: &'static Encoding,
    stack: &mut Vec<usize>,
) -> PResult<bool> {
    if let Value::Ref(r) = v {
        let mut inner = r.get();
        let rec = convert_variable(vm, arg_num, &mut inner, from, to, stack)?;
        r.set(inner);
        return Ok(rec);
    }
    match v {
        Value::String(s) => {
            let out = convert_encoding_ex(vm, s, to, from)?;
            *v = Value::String(PhpStr::from_vec(out));
        }
        Value::Array(a) => {
            let id = Rc::as_ptr(a) as *const () as usize;
            if stack.contains(&id) {
                return Ok(true);
            }
            stack.push(id);
            let arr = Rc::make_mut(a);
            let new_id = arr as *const PhpArray as *const () as usize;
            stack.push(new_id);
            let keys: Vec<ArrayKey> = arr.keys().collect();
            for k in keys {
                let mut e = arr.get(&k).cloned().unwrap_or(Value::Null);
                if convert_variable(vm, arg_num, &mut e, from, to, stack)? {
                    stack.pop();
                    stack.pop();
                    return Ok(true);
                }
                if let Some(slot) = arr.get_mut(&k)
                    && !matches!(slot, Value::Ref(_))
                {
                    *slot = e;
                }
            }
            stack.pop();
            stack.pop();
        }
        Value::Object(o) => {
            vm.deprecated(format!(
                "Passing an object for argument #{arg_num} $vars to mb_convert_variables() is deprecated, call get_object_vars() first instead"
            ))?;
            let o = o.clone();
            let id = o.id() as usize | (1 << 62);
            if stack.contains(&id) {
                return Ok(true);
            }
            stack.push(id);
            for (name, val) in o.properties() {
                let mut e = val.clone();
                if convert_variable(vm, arg_num, &mut e, from, to, stack)? {
                    stack.pop();
                    return Ok(true);
                }
                if !matches!(val, Value::Ref(_)) {
                    o.set_prop(&name, e);
                }
            }
            stack.pop();
        }
        Value::Undef => {}
        other => {
            let ty = other.type_name();
            vm.warning(format!(
                "Argument #{arg_num} must be of type string|array|object or only contain entries of type string|array|object, {ty} given"
            ))?;
        }
    }
    Ok(false)
}

pub fn mb_convert_variables(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let to_name = args.str(0).clone();
    let to = get_encoding(vm, Some(&to_name), 1)?;
    let mut order_significant = true;
    let elist = match args.get(1).clone() {
        Value::Array(a) => {
            if let Some(all) = &globals(vm)?.all_encodings_list
                && Rc::ptr_eq(all, &a)
            {
                order_significant = false;
            }
            parse_encoding_array(vm, &a, 2)?
        }
        Value::String(s) => parse_encoding_list(vm, &s, 2)?,
        _ => unreachable!("ZPP guarantees array|string"),
    };
    if elist.is_empty() {
        return Err(vm.argument_value_error(2, "must specify at least one encoding"));
    }
    let vars: Vec<Value> = std::iter::once(args.get(2).clone())
        .chain(args.rest(3).iter().cloned())
        .collect();

    let from = if elist.len() == 1 {
        elist[0]
    } else {
        let mut strings = Vec::new();
        for v in &vars {
            if find_strings(v, &mut strings, &mut Vec::new()).is_err() {
                vm.warning("Cannot handle recursive references")?;
                return Ok(Value::Bool(false));
            }
        }
        let slices: Vec<&[u8]> = strings.iter().map(|s| s.as_bytes()).collect();
        let strict = strict_detection(vm);
        match guess_encoding_for_strings(&slices, &elist, strict, order_significant) {
            Some(e) => e,
            None => {
                vm.warning("Unable to detect encoding")?;
                return Ok(Value::Bool(false));
            }
        }
    };

    for (n, v) in vars.iter().enumerate() {
        let mut v = v.clone();
        if convert_variable(vm, n + 3, &mut v, from, to, &mut Vec::new())? {
            vm.warning("Cannot handle recursive references")?;
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::from(from.name))
}

// ----- numeric entities ---------------------------------------------------

/// `make_conversion_map`.
fn make_conversion_map(vm: &mut Vm, map: &PhpArray) -> PResult<Vec<u32>> {
    if !map.len().is_multiple_of(4) {
        return Err(vm.argument_value_error(2, "must have a multiple of 4 elements"));
    }
    let mut out = Vec::with_capacity(map.len());
    for v in map.values() {
        match zval_try_get_long(vm, v)? {
            Some(l) => out.push(l as u32),
            None => {
                return Err(
                    vm.argument_value_error(2, "must only be composed of values of type int")
                );
            }
        }
    }
    Ok(out)
}

/// `zval_try_get_long`.
fn zval_try_get_long(vm: &mut Vm, v: &Value) -> PResult<Option<i64>> {
    use php_runtime::value::numeric::{
        Numeric, dval_to_lval_cap, is_long_compatible, is_numeric_string_ex,
    };
    Ok(match v {
        Value::Null | Value::Undef | Value::Bool(false) => Some(0),
        Value::Bool(true) => Some(1),
        Value::Int(i) => Some(*i),
        Value::Float(d) => Some(vm.dval_to_lval_safe(*d)?),
        Value::String(s) => match is_numeric_string_ex(s, true) {
            None => None,
            Some(info) => match info.num {
                Numeric::Int(i) => Some(i),
                Numeric::Float(d) => {
                    let l = dval_to_lval_cap(d);
                    if !is_long_compatible(d, l) {
                        let mut msg = b"Implicit conversion from float-string \"".to_vec();
                        msg.extend_from_slice(s);
                        msg.extend_from_slice(b"\" to int loses precision");
                        vm.engine_deprecated(msg)?;
                    }
                    Some(l)
                }
            },
        },
        Value::Ref(r) => {
            let inner = r.get();
            return zval_try_get_long(vm, &inner);
        }
        Value::Object(o) => match vm.object_cast(o, php_runtime::class::CastKind::Int)? {
            Some(Value::Int(i)) => Some(i),
            _ => None,
        },
        _ => None,
    })
}

/// `html_numeric_entity_convert`.
fn entity_convert(w: u32, convmap: &[u32]) -> Option<u32> {
    convmap.as_chunks::<4>().0.iter().find_map(|m| {
        let (lo, hi, offset, mask) = (m[0], m[1], m[2], m[3]);
        (w >= lo && w <= hi).then(|| w.wrapping_add(offset) & mask)
    })
}

/// `html_numeric_entity_deconvert`.
fn entity_deconvert(number: u32, convmap: &[u32]) -> Option<u32> {
    convmap.as_chunks::<4>().0.iter().find_map(|m| {
        let codepoint = number.wrapping_sub(m[2]);
        (codepoint >= m[0] && codepoint <= m[1]).then_some(codepoint)
    })
}

/// `html_numeric_entity_encode`.
fn numeric_entity_encode(
    input: &[u8],
    encoding: &'static Encoding,
    convmap: &[u32],
    hex: bool,
    illegal: (u32, u32),
) -> Vec<u8> {
    let mut wchar_buf = [0u32; 32];
    let mut converted: Vec<u32> = Vec::with_capacity(32 * 13);
    let mut state = 0u32;
    let mut inp = input;
    let mut buf = ConvertBuf::new(input.len(), illegal.0, illegal.1);
    while !inp.is_empty() {
        let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        converted.clear();
        for &w in &wchar_buf[..n] {
            match entity_convert(w, convmap) {
                Some(w) => {
                    converted.extend([b'&' as u32, b'#' as u32]);
                    if hex {
                        converted.push(b'x' as u32);
                        converted.extend(format!("{w:X}").bytes().map(u32::from));
                    } else {
                        converted.extend(w.to_string().bytes().map(u32::from));
                    }
                    converted.push(b';' as u32);
                }
                None => converted.push(w),
            }
        }
        (encoding.from_wchar)(&converted, &mut buf, inp.is_empty());
    }
    buf.into_bytes()
}

pub fn mb_encode_numericentity(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let map = args.array(1).clone();
    let hex = args.bool(3);
    let encoding = get_encoding(vm, args.str_opt(2), 3)?;
    let convmap = make_conversion_map(vm, &map)?;
    let illegal = illegal_settings(vm)?;
    Ok(Value::String(PhpStr::from_vec(numeric_entity_encode(
        &s, encoding, &convmap, hex, illegal,
    ))))
}

const DEC_ENTITY_MINLEN: usize = 3; // "&#" and 1 decimal digit
const HEX_ENTITY_MINLEN: usize = 4; // "&#x" and 1 hex digit
const DEC_ENTITY_MAXLEN: usize = 12; // "&#" and 10 decimal digits
const HEX_ENTITY_MAXLEN: usize = 11; // "&#x" and 8 hex digits

fn is_digit(w: u32) -> bool {
    (b'0' as u32..=b'9' as u32).contains(&w)
}

fn is_hex_digit(w: u32) -> bool {
    is_digit(w)
        || (b'A' as u32..=b'F' as u32).contains(&w)
        || (b'a' as u32..=b'f' as u32).contains(&w)
}

/// `html_numeric_entity_decode`: an entity cut off at the end of a batch is
/// carried over to the next one.
fn numeric_entity_decode(
    input: &[u8],
    encoding: &'static Encoding,
    convmap: &[u32],
    illegal: (u32, u32),
) -> Vec<u8> {
    const AMP: u32 = b'&' as u32;
    let mut wchar_buf = [0u32; 128];
    let mut converted: Vec<u32> = Vec::with_capacity(128);
    let mut state = 0u32;
    let mut inp = input;
    let mut buf = ConvertBuf::new(input.len(), illegal.0, illegal.1);
    let mut offset = 0usize;

    while !inp.is_empty() {
        // Leave room for the '&' sentinel at the end.
        let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf[offset..127], &mut state);
        let out_len = n + offset;
        wchar_buf[out_len] = AMP;
        let more = !inp.is_empty();
        converted.clear();

        let mut p = 0usize;
        if offset == 0 {
            while wchar_buf[p] != AMP {
                p += 1;
            }
            if p == out_len {
                // No entities in this batch.
                (encoding.from_wchar)(&wchar_buf[..out_len], &mut buf, !more);
                continue;
            }
            converted.extend_from_slice(&wchar_buf[..p]);
        }
        let mut carry: Option<(usize, usize)> = None;
        loop {
            // wchar_buf[p] == '&'
            let mut p2 = p + 1;
            if wchar_buf[p2] == b'#' as u32 {
                p2 += 1;
                if wchar_buf[p2] == b'x' as u32 {
                    // Possible hex entity
                    p2 += 1;
                    while is_hex_digit(wchar_buf[p2]) {
                        p2 += 1;
                    }
                    if p2 == out_len && more && p2 - p <= HEX_ENTITY_MAXLEN {
                        carry = Some((p, p2));
                        break;
                    } else if p2 - p < HEX_ENTITY_MINLEN || p2 - p > HEX_ENTITY_MAXLEN {
                        converted.extend_from_slice(&wchar_buf[p..p2]);
                    } else {
                        let mut value = 0u32;
                        for &w in &wchar_buf[p + 3..p2] {
                            let d = if w <= b'9' as u32 {
                                w - b'0' as u32
                            } else if w >= b'a' as u32 {
                                10 + w - b'a' as u32
                            } else {
                                10 + w - b'A' as u32
                            };
                            value = value.wrapping_mul(16).wrapping_add(d);
                        }
                        match entity_deconvert(value, convmap) {
                            Some(cp) => {
                                converted.push(cp);
                                if wchar_buf[p2] == b';' as u32 {
                                    p2 += 1;
                                }
                            }
                            None => converted.extend_from_slice(&wchar_buf[p..p2]),
                        }
                    }
                } else {
                    // Possible decimal entity
                    while is_digit(wchar_buf[p2]) {
                        p2 += 1;
                    }
                    if p2 == out_len && more && p2 - p <= DEC_ENTITY_MAXLEN {
                        carry = Some((p, p2));
                        break;
                    } else if p2 - p < DEC_ENTITY_MINLEN || p2 - p > DEC_ENTITY_MAXLEN {
                        converted.extend_from_slice(&wchar_buf[p..p2]);
                    } else {
                        let mut value = 0u32;
                        let mut too_big = false;
                        for &w in &wchar_buf[p + 2..p2] {
                            // 0x19999999 is 1/10th of 0xFFFFFFFF.
                            if value > 0x19999999 {
                                too_big = true;
                                break;
                            }
                            value = value * 10 + (w - b'0' as u32);
                        }
                        if too_big {
                            converted.extend_from_slice(&wchar_buf[p..p2]);
                        } else {
                            match entity_deconvert(value, convmap) {
                                Some(cp) => {
                                    converted.push(cp);
                                    if wchar_buf[p2] == b';' as u32 {
                                        p2 += 1;
                                    }
                                }
                                None => converted.extend_from_slice(&wchar_buf[p..p2]),
                            }
                        }
                    }
                }
            } else if p2 == out_len && more {
                // '&' at the end of the batch.
                carry = Some((p, p + 1));
                break;
            } else {
                converted.push(AMP);
            }

            // Copy up to the next '&'.
            p = p2;
            while wchar_buf[p2] != AMP {
                p2 += 1;
            }
            if p2 > p {
                converted.extend_from_slice(&wchar_buf[p..p2]);
                p = p2;
            }
            if p >= out_len {
                break;
            }
        }
        match carry {
            Some((start, end)) => {
                wchar_buf.copy_within(start..end, 0);
                offset = end - start;
            }
            None => offset = 0,
        }
        (encoding.from_wchar)(&converted, &mut buf, !more);
    }
    buf.into_bytes()
}

pub fn mb_decode_numericentity(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let map = args.array(1).clone();
    let encoding = get_encoding(vm, args.str_opt(2), 3)?;
    let convmap = make_conversion_map(vm, &map)?;
    let illegal = illegal_settings(vm)?;
    Ok(Value::String(PhpStr::from_vec(numeric_entity_decode(
        &s, encoding, &convmap, illegal,
    ))))
}

/// The shared `mb_list_encodings()` array (re-exported for tests).
pub fn list_encodings(vm: &mut Vm) -> PResult<Value> {
    Ok(Value::Array(all_encodings_list(vm)?))
}
