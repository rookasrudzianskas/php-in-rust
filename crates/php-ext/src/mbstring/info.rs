//! The settings functions of `mbstring.c`: `mb_language`,
//! `mb_internal_encoding`, `mb_http_input`/`mb_http_output`,
//! `mb_detect_order`, `mb_substitute_character`, `mb_preferred_mime_name`,
//! `mb_get_info`, `mb_list_encodings`, `mb_encoding_aliases`.

use php_runtime::{Args, PResult, PhpArray, PhpStr, Value, Vm};

use super::globals::{
    all_encodings_list, cstr_lossy, encoding_translation, get_encoding, get_encoding_or_pass,
    globals, parse_encoding_array, parse_encoding_list, strict_detection,
};
use super::mbfl::encoding::{
    ILLEGAL_MODE_CHAR, ILLEGAL_MODE_ENTITY, ILLEGAL_MODE_LONG, ILLEGAL_MODE_NONE,
    name2encoding_cstr, no_encoding2name, preferred_mime_name,
};
use super::mbfl::language::{NoLanguage, name2language, no2language};

pub fn mb_language(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(name) = args.str_opt(0).cloned() else {
        let lang = globals(vm)?.language;
        return Ok(Value::from(no2language(lang).name));
    };
    if vm.ini_set("mbstring.language", name.clone()).is_none() {
        // OnUpdate_mbstring_language resets the language before failing.
        globals(vm)?.language = NoLanguage::Neutral;
        return Err(vm.argument_value_error(
            1,
            &format!("must be a valid language, \"{}\" given", cstr_lossy(&name)),
        ));
    }
    let g = globals(vm)?;
    if let Some(l) = name2language(&name) {
        g.language = l.no;
    }
    Ok(Value::Bool(true))
}

pub fn mb_internal_encoding(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(name) = args.str_opt(0).cloned() else {
        return Ok(Value::from(globals(vm)?.current_internal_encoding.name));
    };
    match name2encoding_cstr(&name) {
        Some(e) => {
            let g = globals(vm)?;
            g.current_internal_encoding = e;
            g.internal_encoding_set = true;
            Ok(Value::Bool(true))
        }
        None => Err(vm.argument_value_error(
            1,
            &format!("must be a valid encoding, \"{}\" given", cstr_lossy(&name)),
        )),
    }
}

pub fn mb_http_input(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ty = args.str_opt(0).cloned();
    let g = globals(vm)?;
    let encoding = match ty.as_deref() {
        None => g.http_input_identify,
        Some([t]) => match t.to_ascii_uppercase() {
            b'G' => g.http_input_identify_get,
            b'P' => g.http_input_identify_post,
            b'C' => g.http_input_identify_cookie,
            b'S' => g.http_input_identify_string,
            b'I' => {
                let arr: PhpArray = g.http_input_list.iter().map(|e| Value::from(e.name)).collect();
                return Ok(Value::array(arr));
            }
            b'L' => {
                if g.http_input_list.is_empty() {
                    return Ok(Value::Bool(false));
                }
                let names: Vec<&str> = g.http_input_list.iter().map(|e| e.name).collect();
                return Ok(Value::from(names.join(",")));
            }
            _ => return Err(http_input_type_error(vm)),
        },
        Some(_) => return Err(http_input_type_error(vm)),
    };
    Ok(match encoding {
        Some(e) => Value::from(e.name),
        None => Value::Bool(false),
    })
}

fn http_input_type_error(vm: &mut Vm) -> php_runtime::Flow {
    vm.argument_value_error(1, "must be one of \"G\", \"P\", \"C\", \"S\", \"I\", or \"L\"")
}

pub fn mb_http_output(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(name) = args.str_opt(0).cloned() else {
        return Ok(Value::from(globals(vm)?.current_http_output_encoding.name));
    };
    // Z_PARAM_PATH_OR_NULL
    if name.contains(&0) {
        return Err(vm.argument_value_error(1, "must not contain any null bytes"));
    }
    match get_encoding_or_pass(&name) {
        Some(e) => {
            let g = globals(vm)?;
            g.http_output_set = true;
            g.current_http_output_encoding = e;
            Ok(Value::Bool(true))
        }
        None => Err(vm.argument_value_error(
            1,
            &format!("must be a valid encoding, \"{}\" given", cstr_lossy(&name)),
        )),
    }
}

pub fn mb_detect_order(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let list = match args.get(0).clone() {
        Value::Array(a) => parse_encoding_array(vm, &a, 1)?,
        Value::String(s) => parse_encoding_list(vm, &s, 1)?,
        _ => {
            let g = globals(vm)?;
            let arr: PhpArray = g
                .current_detect_order_list
                .iter()
                .map(|e| Value::from(e.name))
                .collect();
            return Ok(Value::array(arr));
        }
    };
    if list.is_empty() {
        return Err(vm.argument_value_error(1, "must specify at least one encoding"));
    }
    globals(vm)?.current_detect_order_list = list;
    Ok(Value::Bool(true))
}

/// The current substitute character as `mb_substitute_character()` and
/// `mb_get_info()` report it.
fn substitute_character_value(mode: u32, substchar: u32) -> Value {
    match mode {
        ILLEGAL_MODE_NONE => Value::from("none"),
        ILLEGAL_MODE_LONG => Value::from("long"),
        ILLEGAL_MODE_ENTITY => Value::from("entity"),
        _ => Value::Int(substchar as i64),
    }
}

pub fn mb_substitute_character(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match args.get(0).clone() {
        Value::String(s) => {
            let mode = if s.eq_ignore_ascii_case(b"none") {
                ILLEGAL_MODE_NONE
            } else if s.eq_ignore_ascii_case(b"long") {
                ILLEGAL_MODE_LONG
            } else if s.eq_ignore_ascii_case(b"entity") {
                ILLEGAL_MODE_ENTITY
            } else {
                return Err(vm.argument_value_error(
                    1,
                    "must be \"none\", \"long\", \"entity\" or a valid codepoint",
                ));
            };
            globals(vm)?.current_filter_illegal_mode = mode;
            Ok(Value::Bool(true))
        }
        Value::Int(cp) => {
            // php_mb_check_code_point: in Unicode range and not a surrogate.
            if !(0..0x110000).contains(&cp) || (0xD800..=0xDFFF).contains(&cp) {
                return Err(vm.argument_value_error(1, "is not a valid codepoint"));
            }
            let g = globals(vm)?;
            g.current_filter_illegal_mode = ILLEGAL_MODE_CHAR;
            g.current_filter_illegal_substchar = cp as u32;
            Ok(Value::Bool(true))
        }
        _ => {
            let g = globals(vm)?;
            Ok(substitute_character_value(
                g.current_filter_illegal_mode,
                g.current_filter_illegal_substchar,
            ))
        }
    }
}

pub fn mb_preferred_mime_name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let Some(e) = name2encoding_cstr(&name) else {
        return Err(vm.argument_value_error(
            1,
            &format!("must be a valid encoding, \"{}\" given", cstr_lossy(&name)),
        ));
    };
    match preferred_mime_name(e) {
        Some(m) => Ok(Value::from(m)),
        None => {
            vm.warning(format!(
                "No MIME preferred name corresponding to \"{}\"",
                cstr_lossy(&name)
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

fn on_off(b: bool) -> Value {
    Value::from(if b { "On" } else { "Off" })
}

pub fn mb_get_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ty = args.str(0).clone();
    let conv_mimetypes = vm
        .ini_get("mbstring.http_output_conv_mimetypes")
        .unwrap_or_default();
    let strict = strict_detection(vm);
    let translation = encoding_translation(vm);
    let g = globals(vm)?;
    let lang = no2language(g.language);
    let detect_order = || -> PhpArray {
        g.current_detect_order_list
            .iter()
            .map(|e| Value::from(e.name))
            .collect()
    };
    let subst = substitute_character_value(g.current_filter_illegal_mode, g.current_filter_illegal_substchar);

    if ty.eq_ignore_ascii_case(b"all") {
        let mut a = PhpArray::new();
        a.insert_str("internal_encoding", g.current_internal_encoding.name);
        if let Some(e) = g.http_input_identify {
            a.insert_str("http_input", e.name);
        }
        a.insert_str("http_output", g.current_http_output_encoding.name);
        a.insert_str("http_output_conv_mimetypes", conv_mimetypes);
        a.insert_str("mail_charset", no_encoding2name(lang.mail_charset));
        a.insert_str("mail_header_encoding", no_encoding2name(lang.mail_header_encoding));
        a.insert_str("mail_body_encoding", no_encoding2name(lang.mail_body_encoding));
        a.insert_str("illegal_chars", g.illegalchars);
        a.insert_str("encoding_translation", on_off(translation));
        a.insert_str("language", lang.name);
        if !g.current_detect_order_list.is_empty() {
            a.insert_str("detect_order", detect_order());
        }
        a.insert_str("substitute_character", subst);
        a.insert_str("strict_detection", on_off(strict));
        return Ok(Value::array(a));
    }
    let lower = ty.to_ascii_lowercase();
    let value = match lower.as_bytes() {
        b"internal_encoding" => Value::from(g.current_internal_encoding.name),
        b"http_input" => match g.http_input_identify {
            Some(e) => Value::from(e.name),
            None => Value::Null,
        },
        b"http_output" => Value::from(g.current_http_output_encoding.name),
        b"http_output_conv_mimetypes" => Value::String(conv_mimetypes),
        b"mail_charset" => Value::from(no_encoding2name(lang.mail_charset)),
        b"mail_header_encoding" => Value::from(no_encoding2name(lang.mail_header_encoding)),
        b"mail_body_encoding" => Value::from(no_encoding2name(lang.mail_body_encoding)),
        b"illegal_chars" => Value::Int(g.illegalchars),
        b"encoding_translation" => on_off(translation),
        b"language" => Value::from(lang.name),
        b"detect_order" => {
            if g.current_detect_order_list.is_empty() {
                Value::Null
            } else {
                Value::array(detect_order())
            }
        }
        b"substitute_character" => subst,
        b"strict_detection" => on_off(strict),
        _ => Value::Undef,
    };
    if value.is_undef() {
        vm.warning("argument #1 ($type) must be a valid type")?;
        return Ok(Value::Bool(false));
    }
    Ok(value)
}

pub fn mb_list_encodings(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Array(all_encodings_list(vm)?))
}

pub fn mb_encoding_aliases(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name: PhpStr = args.str(0).clone();
    let e = get_encoding(vm, Some(&name), 1)?;
    let arr: PhpArray = e.aliases.iter().map(|a| Value::from(*a)).collect();
    Ok(Value::array(arr))
}
