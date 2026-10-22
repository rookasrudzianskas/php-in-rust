//! `zend_mbstring_globals` (`MBSTRG`) and the ini handlers of `mbstring.c`.
//!
//! The runtime's ini validators cannot reach the VM, so the `on_modify`
//! side effects are applied lazily: every mbstring entry point calls
//! [`globals`], which replays the handlers of directives whose value changed
//! since the last call (in registration order). Values php-src's handlers
//! reject are refused up front by the validators.

use std::cell::Cell;

use php_runtime::ini::{IniDef, modifiable};
use php_runtime::{PResult, PhpArray, PhpArrayRef, PhpStr, Value, Vm};

use super::mbfl::encoding::{
    Encoding, ILLEGAL_MODE_CHAR, ILLEGAL_MODE_ENTITY, ILLEGAL_MODE_LONG, ILLEGAL_MODE_NONE,
    NoEncoding, enc, name2encoding, name2encoding_cstr,
};
use super::mbfl::language::{NoLanguage, name2language};

/// php-src's `php_mb_default_identify_list_*`.
fn default_identify_list(lang: NoLanguage) -> &'static [NoEncoding] {
    use NoEncoding as E;
    match lang {
        NoLanguage::Japanese => &[E::Ascii, E::Jis, E::Utf8, E::EucJp, E::Sjis],
        NoLanguage::Korean => &[E::Ascii, E::Utf8, E::EucKr, E::Uhc],
        NoLanguage::TraditionalChinese => &[E::Ascii, E::Utf8, E::EucTw, E::Big5],
        NoLanguage::SimplifiedChinese => &[E::Ascii, E::Utf8, E::EucCn, E::Cp936, E::Gb18030_2022],
        NoLanguage::Russian => &[E::Ascii, E::Utf8, E::Koi8r, E::Cp1251, E::Cp866],
        NoLanguage::Armenian => &[E::Ascii, E::Utf8, E::Armscii8],
        NoLanguage::Turkish => &[E::Ascii, E::Utf8, E::Cp1254, E::Iso8859_9],
        NoLanguage::Ukrainian => &[E::Ascii, E::Utf8, E::Koi8u],
        _ => &[E::Ascii, E::Utf8],
    }
}

/// The ini directives whose handlers have side effects, in registration
/// order, followed by the core settings `mbstring_internal_encoding_changed_hook`
/// watches.
const WATCHED: [&str; 10] = [
    "mbstring.language",
    "mbstring.detect_order",
    "mbstring.http_input",
    "mbstring.http_output",
    "mbstring.internal_encoding",
    "mbstring.substitute_character",
    "default_charset",
    "internal_encoding",
    "input_encoding",
    "output_encoding",
];

/// `MBSTRG(...)`.
pub struct MbGlobals {
    initialized: bool,
    /// Ini values the state below was derived from (`None` = NULL).
    seen: [Option<Option<PhpStr>>; 10],
    /// [`INI_EVENTS`] counters already handled.
    seen_events: [u64; 6],
    pub language: NoLanguage,
    pub internal_encoding: &'static Encoding,
    pub current_internal_encoding: &'static Encoding,
    pub http_output_encoding: &'static Encoding,
    pub current_http_output_encoding: &'static Encoding,
    pub http_input_identify: Option<&'static Encoding>,
    pub http_input_identify_get: Option<&'static Encoding>,
    pub http_input_identify_post: Option<&'static Encoding>,
    pub http_input_identify_cookie: Option<&'static Encoding>,
    pub http_input_identify_string: Option<&'static Encoding>,
    pub http_input_list: Vec<&'static Encoding>,
    pub detect_order_list: Option<Vec<&'static Encoding>>,
    pub current_detect_order_list: Vec<&'static Encoding>,
    pub default_detect_order_list: &'static [NoEncoding],
    pub filter_illegal_mode: u32,
    pub filter_illegal_substchar: u32,
    pub current_filter_illegal_mode: u32,
    pub current_filter_illegal_substchar: u32,
    pub illegalchars: i64,
    pub outconv_enabled: bool,
    pub outconv_state: u32,
    pub last_used_encoding_name: Option<PhpStr>,
    pub last_used_encoding: Option<&'static Encoding>,
    pub internal_encoding_set: bool,
    pub http_output_set: bool,
    pub http_input_set: bool,
    /// The shared array returned by `mb_list_encodings()`.
    pub all_encodings_list: Option<PhpArrayRef>,
}

impl Default for MbGlobals {
    /// `PHP_GINIT_FUNCTION(mbstring)`.
    fn default() -> MbGlobals {
        let utf8 = enc(NoEncoding::Utf8);
        let pass = enc(NoEncoding::Pass);
        MbGlobals {
            initialized: false,
            seen: Default::default(),
            seen_events: [0; 6],
            language: NoLanguage::Uni,
            internal_encoding: utf8,
            current_internal_encoding: utf8,
            http_output_encoding: pass,
            current_http_output_encoding: pass,
            http_input_identify: None,
            http_input_identify_get: None,
            http_input_identify_post: None,
            http_input_identify_cookie: None,
            http_input_identify_string: None,
            http_input_list: Vec::new(),
            detect_order_list: None,
            current_detect_order_list: Vec::new(),
            default_detect_order_list: default_identify_list(NoLanguage::Neutral),
            filter_illegal_mode: ILLEGAL_MODE_CHAR,
            filter_illegal_substchar: b'?' as u32,
            current_filter_illegal_mode: ILLEGAL_MODE_CHAR,
            current_filter_illegal_substchar: b'?' as u32,
            illegalchars: 0,
            outconv_enabled: false,
            outconv_state: 0,
            last_used_encoding_name: None,
            last_used_encoding: None,
            internal_encoding_set: false,
            http_output_set: false,
            http_input_set: false,
            all_encodings_list: None,
        }
    }
}

/// `php_mb_parse_encoding_list` without diagnostics: `Err(name)` for the
/// first invalid encoding.
pub fn parse_encoding_list_raw(
    value: &[u8],
    default_list: &[NoEncoding],
) -> Result<Vec<&'static Encoding>, Vec<u8>> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut v = value;
    if v.len() > 2 && v[0] == b'"' && v[v.len() - 1] == b'"' {
        v = &v[1..v.len() - 1];
    }
    // php-src sizes the list as 1 + commas + |default list| and stops there.
    let size = 1 + v.iter().filter(|&&c| c == b',').count() + default_list.len();
    let mut list = Vec::new();
    let mut included_auto = false;
    let mut rest = v;
    loop {
        let comma = rest.iter().position(|&c| c == b',');
        let mut item = match comma {
            Some(i) => &rest[..i],
            None => rest,
        };
        while let Some((&c, tail)) = item.split_first() {
            if c == b' ' || c == b'\t' {
                item = tail;
            } else {
                break;
            }
        }
        while let Some((&c, head)) = item.split_last() {
            if c == b' ' || c == b'\t' {
                item = head;
            } else {
                break;
            }
        }
        if item.eq_ignore_ascii_case(b"auto") {
            if !included_auto {
                included_auto = true;
                list.extend(default_list.iter().map(|&n| enc(n)));
            }
        } else {
            match name2encoding(item) {
                Some(e) => list.push(e),
                None => return Err(item.to_vec()),
            }
        }
        match comma {
            Some(i) if list.len() < size => rest = &rest[i + 1..],
            _ => break,
        }
    }
    Ok(list)
}

/// `php_mb_parse_encoding_list` in function context (ValueError on failure).
pub fn parse_encoding_list(
    vm: &mut Vm,
    value: &[u8],
    arg_num: usize,
) -> PResult<Vec<&'static Encoding>> {
    let default_list = globals(vm)?.default_detect_order_list;
    match parse_encoding_list_raw(value, default_list) {
        Ok(l) => Ok(l),
        Err(name) => Err(vm.argument_value_error(
            arg_num,
            &format!(
                "contains invalid encoding \"{}\"",
                String::from_utf8_lossy(&name)
            ),
        )),
    }
}

/// `php_mb_parse_encoding_array`.
pub fn parse_encoding_array(
    vm: &mut Vm,
    arr: &PhpArray,
    arg_num: usize,
) -> PResult<Vec<&'static Encoding>> {
    let default_list = globals(vm)?.default_detect_order_list;
    let mut list = Vec::new();
    let mut included_auto = false;
    for (_, v) in arr.iter() {
        let s = vm.to_php_string(v)?;
        if s.eq_ignore_ascii_case(b"auto") {
            if !included_auto {
                included_auto = true;
                list.extend(default_list.iter().map(|&n| enc(n)));
            }
        } else {
            match name2encoding_cstr(&s) {
                Some(e) => list.push(e),
                None => {
                    return Err(vm.argument_value_error(
                        arg_num,
                        &format!("contains invalid encoding \"{}\"", cstr_lossy(&s)),
                    ));
                }
            }
        }
    }
    Ok(list)
}

/// A C string (up to the first NUL) for messages.
pub fn cstr_lossy(s: &[u8]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).into_owned()
}

/// `php_mb_get_encoding_or_pass`.
pub fn get_encoding_or_pass(name: &[u8]) -> Option<&'static Encoding> {
    if name == b"pass" {
        return Some(enc(NoEncoding::Pass));
    }
    name2encoding(name)
}

/// `php_get_internal_encoding` & co. (`main/main.c`).
fn core_encoding(vm: &Vm, name: &str) -> Vec<u8> {
    if let Some(v) = vm.ini_get(name).filter(|v| !v.is_empty()) {
        return v.to_vec();
    }
    if let Some(v) = vm.ini_get("default_charset").filter(|v| !v.is_empty()) {
        return v.to_vec();
    }
    b"UTF-8".to_vec()
}

impl MbGlobals {
    /// `OnUpdate_mbstring_language`.
    fn on_language(&mut self, v: Option<&[u8]>) -> bool {
        match v.and_then(name2language) {
            Some(l) => {
                self.language = l.no;
                self.default_detect_order_list = default_identify_list(l.no);
                true
            }
            None => {
                self.language = NoLanguage::Neutral;
                false
            }
        }
    }

    /// `OnUpdate_mbstring_detect_order`.
    fn on_detect_order(&mut self, v: Option<&[u8]>, runtime: bool) -> bool {
        match v {
            None => {
                self.detect_order_list = None;
                true
            }
            Some(v) => match parse_encoding_list_raw(v, self.default_detect_order_list) {
                Ok(list) if !list.is_empty() => {
                    self.detect_order_list = Some(list);
                    if runtime {
                        self.populate_current_detect_order_list();
                    }
                    true
                }
                _ => false,
            },
        }
    }

    /// `_php_mb_ini_mbstring_http_input_set`.
    fn http_input_set_value(&mut self, v: &[u8]) -> bool {
        if v == b"pass" {
            self.http_input_list = vec![enc(NoEncoding::Pass)];
            return true;
        }
        match parse_encoding_list_raw(v, self.default_detect_order_list) {
            Ok(list) if !list.is_empty() => {
                self.http_input_list = list;
                true
            }
            _ => false,
        }
    }

    /// `OnUpdate_mbstring_http_input`.
    fn on_http_input(&mut self, vm: &Vm, v: Option<&[u8]>) -> bool {
        match v.filter(|v| !v.is_empty()) {
            None => {
                self.http_input_set = false;
                let e = core_encoding(vm, "input_encoding");
                self.http_input_set_value(&e);
                true
            }
            Some(v) => {
                self.http_input_set = true;
                self.http_input_set_value(v)
            }
        }
    }

    /// `_php_mb_ini_mbstring_http_output_set`.
    fn http_output_set_value(&mut self, v: &[u8]) -> bool {
        match get_encoding_or_pass(v) {
            Some(e) => {
                self.http_output_encoding = e;
                self.current_http_output_encoding = e;
                true
            }
            None => false,
        }
    }

    /// `OnUpdate_mbstring_http_output`.
    fn on_http_output(&mut self, vm: &Vm, v: Option<&[u8]>) -> bool {
        match v.filter(|v| !v.is_empty()) {
            None => {
                self.http_output_set = false;
                let e = core_encoding(vm, "output_encoding");
                self.http_output_set_value(&e);
                true
            }
            Some(v) => {
                self.http_output_set = true;
                self.http_output_set_value(v)
            }
        }
    }

    /// `_php_mb_ini_mbstring_internal_encoding_set`: unknown names fall back
    /// to UTF-8.
    fn internal_encoding_set_value(&mut self, v: &[u8]) {
        let e = if v.is_empty() {
            None
        } else {
            name2encoding_cstr(v)
        };
        let e = e.unwrap_or_else(|| enc(NoEncoding::Utf8));
        self.internal_encoding = e;
        self.current_internal_encoding = e;
    }

    /// `OnUpdate_mbstring_internal_encoding`.
    fn on_internal_encoding(&mut self, vm: &Vm, v: Option<&[u8]>) {
        match v.filter(|v| !v.is_empty()) {
            Some(v) => {
                self.internal_encoding_set = true;
                self.internal_encoding_set_value(v);
            }
            None => {
                self.internal_encoding_set = false;
                let e = core_encoding(vm, "internal_encoding");
                self.internal_encoding_set_value(&e);
            }
        }
    }

    /// `OnUpdate_mbstring_substitute_character`.
    fn on_substitute_character(&mut self, v: Option<&[u8]>) {
        match v {
            Some(v) => {
                let mode = if v.eq_ignore_ascii_case(b"none") {
                    ILLEGAL_MODE_NONE
                } else if v.eq_ignore_ascii_case(b"long") {
                    ILLEGAL_MODE_LONG
                } else if v.eq_ignore_ascii_case(b"entity") {
                    ILLEGAL_MODE_ENTITY
                } else {
                    ILLEGAL_MODE_CHAR
                };
                self.filter_illegal_mode = mode;
                self.current_filter_illegal_mode = mode;
                if mode == ILLEGAL_MODE_CHAR && !v.is_empty() {
                    let (c, used) = php_runtime::ini::strtol(v, 0);
                    // `*endptr == '\0'`: the whole (C) string was consumed.
                    let rest = &v[used..];
                    if rest.first().is_none_or(|&b| b == 0) {
                        self.filter_illegal_substchar = c as i32 as u32;
                        self.current_filter_illegal_substchar = c as i32 as u32;
                    }
                }
            }
            None => {
                self.filter_illegal_mode = ILLEGAL_MODE_CHAR;
                self.current_filter_illegal_mode = ILLEGAL_MODE_CHAR;
                self.filter_illegal_substchar = b'?' as u32;
                self.current_filter_illegal_substchar = b'?' as u32;
            }
        }
    }

    /// `mbstring_internal_encoding_changed_hook`.
    fn internal_encoding_changed(&mut self, vm: &Vm) {
        if !self.internal_encoding_set {
            let e = core_encoding(vm, "internal_encoding");
            self.internal_encoding_set_value(&e);
        }
        if !self.http_output_set {
            let e = core_encoding(vm, "output_encoding");
            self.http_output_set_value(&e);
        }
        if !self.http_input_set {
            let e = core_encoding(vm, "input_encoding");
            self.http_input_set_value(&e);
        }
    }

    /// `php_mb_populate_current_detect_order_list`.
    pub fn populate_current_detect_order_list(&mut self) {
        self.current_detect_order_list = match &self.detect_order_list {
            Some(l) if !l.is_empty() => l.clone(),
            _ => self
                .default_detect_order_list
                .iter()
                .map(|&n| enc(n))
                .collect(),
        };
    }

    /// Runs the handler of watched directive `i`; `false` is FAILURE.
    fn apply(&mut self, vm: &Vm, i: usize, v: Option<&[u8]>, runtime: bool) -> bool {
        match i {
            0 => self.on_language(v),
            1 => self.on_detect_order(v, runtime),
            2 => self.on_http_input(vm, v),
            3 => self.on_http_output(vm, v),
            4 => {
                self.on_internal_encoding(vm, v);
                true
            }
            5 => {
                self.on_substitute_character(v);
                true
            }
            _ => {
                self.internal_encoding_changed(vm);
                true
            }
        }
    }
}

fn ini_value(vm: &Vm, name: &str) -> Option<PhpStr> {
    vm.ini.get(name.as_bytes()).and_then(|e| e.value.clone())
}

/// The registered default of a watched mbstring directive.
fn ini_default(name: &str) -> Option<&'static str> {
    INI_ENTRIES
        .iter()
        .find(|d| d.name == name)
        .and_then(|d| d.default)
}

/// `MBSTRG`: the per-VM state, synchronised with the ini settings.
pub fn globals(vm: &mut Vm) -> PResult<&mut MbGlobals> {
    let mut g = std::mem::take(vm.ext_data::<MbGlobals>());
    if !g.initialized {
        g.initialized = true;
        // MINIT: REGISTER_INI_ENTRIES runs every handler with the configured
        // value; a value the handler rejects is replaced by the default.
        for (i, name) in WATCHED.iter().enumerate().take(6) {
            let v = ini_value(vm, name);
            if !g.apply(vm, i, v.as_deref(), false) {
                let def = ini_default(name);
                vm.ini_set_system(name.as_bytes(), def.unwrap_or(""));
                if def.is_none() {
                    vm.ini.set_default(name.as_bytes(), None);
                }
                g.apply(vm, i, def.map(str::as_bytes), false);
            }
        }
        g.internal_encoding_changed(vm);
        // RINIT.
        g.current_internal_encoding = g.internal_encoding;
        g.current_http_output_encoding = g.http_output_encoding;
        g.current_filter_illegal_mode = g.filter_illegal_mode;
        g.current_filter_illegal_substchar = g.filter_illegal_substchar;
        g.illegalchars = 0;
        g.populate_current_detect_order_list();
        for (i, name) in WATCHED.iter().enumerate() {
            g.seen[i] = Some(ini_value(vm, name));
        }
        g.seen_events = INI_EVENTS.with(|e| e.get().map(|(n, _)| n));
    } else {
        let mut core_changed = false;
        let events = INI_EVENTS.with(Cell::get);
        for (i, name) in WATCHED.iter().enumerate() {
            let v = ini_value(vm, name);
            let changed = g.seen[i].as_ref() != Some(&v);
            if i < 6 {
                // php-src runs the handler on every ini_set, even when the
                // value does not change.
                let (count, ok) = events[i];
                if changed || count != g.seen_events[i] {
                    if ok || changed {
                        g.apply(vm, i, v.as_deref(), true);
                    } else if i == 0 {
                        // OnUpdate_mbstring_language resets before failing.
                        g.language = NoLanguage::Neutral;
                    }
                }
                g.seen_events[i] = count;
            } else if changed {
                core_changed = true;
            }
            g.seen[i] = Some(v);
        }
        if core_changed {
            g.internal_encoding_changed(vm);
        }
    }
    *vm.ext_data::<MbGlobals>() = g;
    Ok(vm.ext_data::<MbGlobals>())
}

/// `MBSTRG(strict_detection)`.
pub fn strict_detection(vm: &Vm) -> bool {
    vm.ini_bool("mbstring.strict_detection")
}

/// `MBSTRG(encoding_translation)`.
pub fn encoding_translation(vm: &Vm) -> bool {
    vm.ini_bool("mbstring.encoding_translation")
}

/// `php_mb_get_encoding`: the named encoding (ValueError for unknown names,
/// deprecations for the non-text encodings) or the current internal one.
pub fn get_encoding(
    vm: &mut Vm,
    name: Option<&PhpStr>,
    arg_num: usize,
) -> PResult<&'static Encoding> {
    let Some(name) = name else {
        return Ok(globals(vm)?.current_internal_encoding);
    };
    {
        let g = globals(vm)?;
        if let (Some(last), Some(e)) = (&g.last_used_encoding_name, g.last_used_encoding)
            && last.eq_ignore_ascii_case(name)
        {
            return Ok(e);
        }
    }
    let Some(e) = name2encoding_cstr(name) else {
        return Err(vm.argument_value_error(
            arg_num,
            &format!("must be a valid encoding, \"{}\" given", cstr_lossy(name)),
        ));
    };
    match e.no {
        NoEncoding::Base64 => vm.deprecated(
            "Handling Base64 via mbstring is deprecated; use base64_encode/base64_decode instead",
        )?,
        NoEncoding::Qprint => vm.deprecated(
            "Handling QPrint via mbstring is deprecated; use quoted_printable_encode/quoted_printable_decode instead",
        )?,
        NoEncoding::HtmlEnt => vm.deprecated(
            "Handling HTML entities via mbstring is deprecated; use htmlspecialchars, htmlentities, or mb_encode_numericentity/mb_decode_numericentity instead",
        )?,
        NoEncoding::Uuencode => vm.deprecated(
            "Handling Uuencode via mbstring is deprecated; use convert_uuencode/convert_uudecode instead",
        )?,
        _ => {}
    }
    let g = globals(vm)?;
    g.last_used_encoding_name = Some(name.clone());
    g.last_used_encoding = Some(e);
    Ok(e)
}

/// The current substitution settings (`MBSTRG(current_filter_illegal_*)`).
pub fn illegal_settings(vm: &mut Vm) -> PResult<(u32, u32)> {
    let g = globals(vm)?;
    Ok((
        g.current_filter_illegal_substchar,
        g.current_filter_illegal_mode,
    ))
}

// ----- ini entries ------------------------------------------------------

thread_local! {
    /// Per watched mbstring directive: how often runtime changes were
    /// attempted (validators run on every `ini_set`) and whether the last
    /// attempt was accepted. Lets [`globals`] replay handlers for `ini_set`
    /// calls that do not change the value.
    static INI_EVENTS: Cell<[(u64, bool); 6]> = const { Cell::new([(0, true); 6]) };
}

fn record(i: usize, ok: bool) -> bool {
    INI_EVENTS.with(|e| {
        let mut v = e.get();
        v[i] = (v[i].0 + 1, ok);
        e.set(v);
    });
    ok
}

fn validate_language(v: &[u8]) -> bool {
    record(0, name2language(v).is_some())
}

fn detect_order_valid(v: &[u8]) -> bool {
    // The default list does not matter for validity.
    matches!(parse_encoding_list_raw(v, &[NoEncoding::Ascii]), Ok(l) if !l.is_empty())
}

fn validate_detect_order(v: &[u8]) -> bool {
    record(1, detect_order_valid(v))
}

fn validate_http_input(v: &[u8]) -> bool {
    record(2, v.is_empty() || v == b"pass" || detect_order_valid(v))
}

fn validate_http_output(v: &[u8]) -> bool {
    record(3, v.is_empty() || get_encoding_or_pass(v).is_some())
}

fn validate_internal_encoding(_v: &[u8]) -> bool {
    record(4, true)
}

fn validate_substitute_character(_v: &[u8]) -> bool {
    record(5, true)
}

/// `PHP_INI_BEGIN()` of mbstring.c (without the mbregex entries).
pub static INI_ENTRIES: [IniDef; 9] = [
    IniDef::new("mbstring.language", Some("neutral"), modifiable::ALL).validated(validate_language),
    IniDef::new("mbstring.detect_order", None, modifiable::ALL).validated(validate_detect_order),
    IniDef::new("mbstring.http_input", None, modifiable::ALL).validated(validate_http_input),
    IniDef::new("mbstring.http_output", None, modifiable::ALL).validated(validate_http_output),
    IniDef::new("mbstring.internal_encoding", None, modifiable::ALL)
        .validated(validate_internal_encoding),
    IniDef::new("mbstring.substitute_character", None, modifiable::ALL)
        .validated(validate_substitute_character),
    IniDef::new(
        "mbstring.encoding_translation",
        Some("0"),
        modifiable::SYSTEM | modifiable::PERDIR,
    ),
    IniDef::new(
        "mbstring.http_output_conv_mimetypes",
        Some("^(text/|application/xhtml\\+xml)"),
        modifiable::ALL,
    ),
    IniDef::new("mbstring.strict_detection", Some("0"), modifiable::ALL),
];

/// The array of encoding names for `mb_list_encodings()`.
pub fn all_encodings_list(vm: &mut Vm) -> PResult<PhpArrayRef> {
    let g = globals(vm)?;
    if let Some(a) = &g.all_encodings_list {
        return Ok(a.clone());
    }
    let arr: PhpArray = super::mbfl::encoding::ENCODING_LIST
        .iter()
        .map(|e| Value::from(e.name))
        .collect();
    let rc = PhpArrayRef::new(arr);
    g.all_encodings_list = Some(rc.clone());
    Ok(rc)
}
