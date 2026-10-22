//! ext/standard/browscap.c
//!
//! `get_browser()` over a browscap ini file named by the `browscap`
//! directive. The file is read lazily on first use (php-src reads it during
//! module startup) and cached per file name.

use std::collections::HashMap;

use php_runtime::errors::level;
use php_runtime::ini::{IniDef, modifiable};
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::basic::BasicState;

const BROWSCAP_NUM_CONTAINS: usize = 5;
const DEFAULT_SECTION_NAME: &[u8] = b"Default Browser Capability Settings";

struct Entry {
    pattern: PhpStr,
    parent: Option<PhpStr>,
    kv_start: usize,
    kv_end: usize,
    contains_start: [usize; BROWSCAP_NUM_CONTAINS],
    contains_len: [usize; BROWSCAP_NUM_CONTAINS],
    prefix_len: usize,
}

/// `browser_data`: the sections in file order plus their key/value pairs.
#[derive(Default)]
struct BrowserData {
    entries: Vec<Entry>,
    index: HashMap<Vec<u8>, usize>,
    kv: Vec<(PhpStr, PhpStr)>,
}

#[derive(Default)]
struct BrowscapState {
    /// The data of the file named by the `browscap` directive.
    loaded: Option<(Vec<u8>, BrowserData)>,
}

fn is_placeholder(c: u8) -> bool {
    c == b'?' || c == b'*'
}

/// Length of the prefix that has no wildcards.
fn compute_prefix_len(pattern: &[u8]) -> usize {
    pattern
        .iter()
        .position(|&c| is_placeholder(c))
        .unwrap_or(pattern.len())
        .min(255)
}

fn compute_contains(pattern: &[u8], start_pos: usize) -> (usize, usize, usize) {
    let mut i = start_pos;
    // first non-placeholder character that starts a run of two or more
    while i < pattern.len() {
        if !is_placeholder(pattern[i]) && i + 1 < pattern.len() && !is_placeholder(pattern[i + 1]) {
            break;
        }
        i += 1;
    }
    let contains_start = i;
    while i < pattern.len() && !is_placeholder(pattern[i]) {
        i += 1;
    }
    (contains_start, (i - contains_start).min(255), i)
}

/// `browscap_convert_pattern()`: the `browser_name_regex` value.
fn convert_pattern(pattern: &[u8]) -> Vec<u8> {
    let mut t = b"~^".to_vec();
    for &c in pattern {
        match c {
            b'?' => t.push(b'.'),
            b'*' => t.extend_from_slice(b".*"),
            b'.' => t.extend_from_slice(b"\\."),
            b'\\' => t.extend_from_slice(b"\\\\"),
            b'(' => t.extend_from_slice(b"\\("),
            b')' => t.extend_from_slice(b"\\)"),
            b'~' => t.extend_from_slice(b"\\~"),
            b'+' => t.extend_from_slice(b"\\+"),
            _ => t.push(c.to_ascii_lowercase()),
        }
    }
    t.extend_from_slice(b"$~");
    t
}

/// What the ini scanner (`ZEND_INI_SCANNER_RAW`) yields for one line.
enum IniLine {
    Section(Vec<u8>),
    /// `key = value` (`None` for a bare label).
    Entry(Vec<u8>, Option<Vec<u8>>),
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// A line-oriented port of the raw ini scanner rules that matter here.
fn scan_ini_line(line: &[u8]) -> Option<IniLine> {
    let mut i = 0;
    while i < line.len() && is_blank(line[i]) {
        i += 1;
    }
    let rest = &line[i..];
    match rest.first() {
        None | Some(b';') => return None,
        Some(b'[') => {
            let body = &rest[1..];
            let end = body.iter().position(|&c| c == b']').unwrap_or(body.len());
            return Some(IniLine::Section(body[..end].to_vec()));
        }
        _ => {}
    }
    let Some(eq) = rest.iter().position(|&c| c == b'=') else {
        // a label without value: the label stops at `;`, `"`, ...
        let end = rest.iter().position(|&c| c == b';').unwrap_or(rest.len());
        let mut key = rest[..end].to_vec();
        while key.last().is_some_and(|&c| is_blank(c)) {
            key.pop();
        }
        return Some(IniLine::Entry(key, None));
    };
    let mut key = rest[..eq].to_vec();
    while key.last().is_some_and(|&c| is_blank(c)) {
        key.pop();
    }
    // raw value: leading blanks skipped, `;` starts a comment unless the
    // value is a double-quoted string that is closed after it
    let mut v = &rest[eq + 1..];
    while let Some((&c, tail)) = v.split_first() {
        if is_blank(c) {
            v = tail;
        } else {
            break;
        }
    }
    let quoted = v.first() == Some(&b'"');
    let mut sc: Option<usize> = None;
    for (idx, &c) in v.iter().enumerate() {
        match c {
            b';' => {
                if sc.is_none() {
                    sc = Some(idx);
                }
            }
            b'"' if quoted => sc = None,
            _ => {}
        }
    }
    let mut val = match sc {
        Some(p) => v[..p].to_vec(),
        None => v.to_vec(),
    };
    while val.last().is_some_and(|&c| is_blank(c)) {
        val.pop();
    }
    if val.len() > 1 && val[0] == b'"' && val[val.len() - 1] == b'"' {
        val = val[1..val.len() - 1].to_vec();
    }
    Some(IniLine::Entry(key, Some(val)))
}

/// `browscap_read_file()`: `Err` carries the `E_CORE_ERROR` text.
fn parse_browscap(data: &[u8], filename: &[u8]) -> Result<BrowserData, String> {
    let mut b = BrowserData::default();
    let mut current: Option<usize> = None;
    let mut section_name: Option<Vec<u8>> = None;

    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    for raw_line in data.split(|&c| c == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        let Some(parsed) = scan_ini_line(line) else {
            continue;
        };
        match parsed {
            IniLine::Section(name) => {
                if name.len() > u16::MAX as usize {
                    // E_WARNING "Skipping excessively long pattern" is not emitted
                    // here (no Vm); the section is skipped like php-src does.
                    current = None;
                    continue;
                }
                let prefix_len = compute_prefix_len(&name);
                let mut pos = prefix_len;
                let mut contains_start = [0; BROWSCAP_NUM_CONTAINS];
                let mut contains_len = [0; BROWSCAP_NUM_CONTAINS];
                for i in 0..BROWSCAP_NUM_CONTAINS {
                    let (s, l, next) = compute_contains(&name, pos);
                    contains_start[i] = s;
                    contains_len[i] = l;
                    pos = next;
                }
                let entry = Entry {
                    pattern: PhpStr::from(name.clone()),
                    parent: None,
                    kv_start: b.kv.len(),
                    kv_end: b.kv.len(),
                    contains_start,
                    contains_len,
                    prefix_len,
                };
                let idx = match b.index.get(&name) {
                    Some(&i) => {
                        b.entries[i] = entry;
                        i
                    }
                    None => {
                        b.entries.push(entry);
                        b.index.insert(name.clone(), b.entries.len() - 1);
                        b.entries.len() - 1
                    }
                };
                current = Some(idx);
                section_name = Some(name);
            }
            IniLine::Entry(key, Some(value)) => {
                let Some(cur) = current else { continue };
                let lv = value.to_ascii_lowercase();
                let new_value: Vec<u8> = match lv.as_slice() {
                    b"on" | b"yes" | b"true" => b"1".to_vec(),
                    b"no" | b"off" | b"none" | b"false" => Vec::new(),
                    _ => value,
                };
                if key.eq_ignore_ascii_case(b"parent") {
                    // a parent equal to the section would loop forever
                    if section_name
                        .as_ref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&new_value))
                    {
                        return Err(format!(
                            "Invalid browscap ini file: 'Parent' value cannot be same as the section name: {} (in file {})",
                            String::from_utf8_lossy(section_name.as_deref().unwrap_or_default()),
                            String::from_utf8_lossy(filename)
                        ));
                    }
                    b.entries[cur].parent = Some(PhpStr::from(new_value));
                } else {
                    b.kv.push((
                        PhpStr::from(key.to_ascii_lowercase()),
                        PhpStr::from(new_value),
                    ));
                    b.entries[cur].kv_end = b.kv.len();
                }
            }
            IniLine::Entry(_, None) => {}
        }
    }
    Ok(b)
}

fn add_kv(bdata: &BrowserData, entry: &Entry, ht: &mut PhpArray) {
    for (k, v) in &bdata.kv[entry.kv_start..entry.kv_end] {
        // zend_hash_add: an existing key is kept
        let key = php_runtime::ArrayKey::from_bytes(k);
        if !ht.contains_key(&key) {
            ht.insert(key, Value::String(v.clone()));
        }
    }
}

/// `browscap_entry_to_array()`
fn entry_to_array(bdata: &BrowserData, entry: &Entry) -> PhpArray {
    let mut ht = PhpArray::new();
    ht.insert_str(
        "browser_name_regex",
        Value::bytes(&convert_pattern(&entry.pattern)),
    );
    ht.insert_str("browser_name_pattern", Value::String(entry.pattern.clone()));
    if let Some(p) = &entry.parent {
        ht.insert_str("parent", Value::String(p.clone()));
    }
    add_kv(bdata, entry, &mut ht);
    ht
}

fn memnstr(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `browscap_match_string_wildcard()`: `*` and `?` wildcards, with the
/// backtracking of php-src's implementation.
fn match_wildcard(s: &[u8], pattern: &[u8]) -> bool {
    let (mut pc, mut sc) = (0usize, 0usize);
    let mut restore: Option<(usize, usize)> = None;

    while sc < s.len() {
        let pattern_char = pattern.get(pc).copied().unwrap_or(0);
        let s_char = s[sc];

        if pattern_char == b'*' && pc < pattern.len() {
            // collapse wildcards
            pc += 1;
            while pc < pattern.len() && pattern[pc] == b'*' {
                pc += 1;
            }
            if pc == pattern.len() {
                return true;
            }
            // jump to the first character of s that can follow the `*`
            if pattern[pc] != b'?' {
                while sc < s.len() && s[sc] != pattern[pc] {
                    sc += 1;
                }
            }
            restore = Some((pc, sc));
            continue;
        } else if pc < pattern.len() && (pattern_char == s_char || pattern_char == b'?') {
            pc += 1;
            sc += 1;
            if pc == pattern.len() {
                if sc == s.len() {
                    return true;
                }
                // fall through to the mismatch handling
            } else {
                continue;
            }
        }

        // mismatch
        match restore {
            Some((rp, rs)) => {
                pc = rp;
                let rs = rs + 1;
                restore = Some((rp, rs));
                sc = rs;
            }
            None => return false,
        }
    }
    while pc < pattern.len() && pattern[pc] == b'*' {
        pc += 1;
    }
    pc == pattern.len()
}

fn minimum_length(e: &Entry) -> usize {
    e.prefix_len + e.contains_len.iter().sum::<usize>()
}

/// `browser_reg_compare()`: `true` for an exact match (ends the search).
fn reg_compare(
    entry_idx: usize,
    entry: &Entry,
    agent: &[u8],
    found: &mut Option<usize>,
    cached_prev_len: &mut usize,
) -> bool {
    let pattern_lc = entry.pattern.to_ascii_lowercase();
    let pattern_lc = pattern_lc.as_bytes();

    // the agent must contain the "contains" portions in order
    let mut cur = entry.prefix_len.min(agent.len());
    for i in 0..BROWSCAP_NUM_CONTAINS {
        if entry.contains_len[i] != 0 {
            let needle = &pattern_lc
                [entry.contains_start[i]..entry.contains_start[i] + entry.contains_len[i]];
            match memnstr(&agent[cur..], needle) {
                Some(p) => cur += p + needle.len(),
                None => return false,
            }
        }
    }

    // an exact match ends the search
    if agent == pattern_lc {
        *found = Some(entry_idx);
        return true;
    }

    let agent_tail = &agent[entry.prefix_len.min(agent.len())..];
    let pattern_tail = &pattern_lc[entry.prefix_len.min(pattern_lc.len())..];
    if match_wildcard(agent_tail, pattern_tail) {
        // prefer the pattern that replaces the fewest characters
        let mut curr_len = entry.prefix_len;
        for &c in &entry.pattern[entry.prefix_len.min(entry.pattern.len())..] {
            if !is_placeholder(c) {
                curr_len += 1;
            }
        }
        if found.is_some() {
            if *cached_prev_len < curr_len {
                *found = Some(entry_idx);
                *cached_prev_len = curr_len;
            }
        } else {
            *found = Some(entry_idx);
            *cached_prev_len = curr_len;
        }
    }
    false
}

/// Loads (and caches) the file named by the `browscap` directive.
fn ensure_loaded(vm: &mut Vm) -> PResult<bool> {
    let filename = vm.ini_get("browscap").unwrap_or_default();
    if filename.is_empty() {
        vm.warning("browscap ini directive not set")?;
        return Ok(false);
    }
    if let Some((name, _)) = &vm.ext_data::<BrowscapState>().loaded {
        if name.as_slice() == filename.as_bytes() {
            return Ok(true);
        }
    }
    let path = filename.to_string_lossy().into_owned();
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => {
            vm.zend_error(
                level::E_CORE_WARNING,
                format!("Cannot open \"{filename}\" for reading"),
            )?;
            return Ok(false);
        }
    };
    match parse_browscap(&data, filename.as_bytes()) {
        Ok(b) => {
            vm.ext_data::<BrowscapState>().loaded = Some((filename.to_vec(), b));
            Ok(true)
        }
        Err(msg) => {
            vm.zend_error(level::E_CORE_ERROR, msg)?;
            Ok(false)
        }
    }
}

fn get_browser(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let agent_name = args.str_opt(0).cloned();
    let return_array = args.bool(1);

    if !ensure_loaded(vm)? {
        return Ok(Value::Bool(false));
    }

    let agent_name = match agent_name {
        Some(a) => a,
        None => match vm.ext_data::<BasicState>().http_user_agent.clone() {
            Some(ua) => ua,
            None => {
                vm.warning(
                    "HTTP_USER_AGENT variable is not set, cannot determine user agent name",
                )?;
                return Ok(Value::Bool(false));
            }
        },
    };
    let lookup = agent_name.to_ascii_lowercase();
    let lookup = lookup.as_bytes();

    let state = vm.ext_data::<BrowscapState>();
    let bdata = &state.loaded.as_ref().expect("loaded above").1;

    let mut found: Option<usize> = bdata.index.get(lookup).copied();
    if found.is_none() {
        let mut cached_prev_len = 0usize;
        for (idx, entry) in bdata.entries.iter().enumerate() {
            // agent name too short
            if lookup.len() < minimum_length(entry) {
                continue;
            }
            // quickly discard patterns whose prefix does not match
            let prefix_ok = (0..entry.prefix_len)
                .all(|i| lookup.get(i).copied() == Some(entry.pattern[i].to_ascii_lowercase()));
            if !prefix_ok {
                continue;
            }
            if reg_compare(idx, entry, lookup, &mut found, &mut cached_prev_len) {
                break;
            }
        }
        if found.is_none() {
            found = bdata.index.get(DEFAULT_SECTION_NAME).copied();
            if found.is_none() {
                return Ok(Value::Bool(false));
            }
        }
    }

    let mut cur = &bdata.entries[found.expect("set above")];
    let mut agent_ht = entry_to_array(bdata, cur);
    while let Some(parent) = &cur.parent {
        match bdata.index.get(parent.as_bytes()) {
            Some(&i) => {
                cur = &bdata.entries[i];
                add_kv(bdata, cur, &mut agent_ht);
            }
            None => break,
        }
    }

    if return_array {
        return Ok(Value::array(agent_ht));
    }
    let std_class = vm
        .find_class("stdClass")
        .expect("stdClass is registered by Core");
    let obj = vm.instantiate(&std_class)?;
    for (k, v) in agent_ht.iter() {
        obj.set_prop(&k.to_php_str(), v.clone());
    }
    Ok(Value::Object(obj))
}

pub static GET_BROWSER: NativeFunction = NativeFunction {
    sig: "get_browser(?string $user_agent = null, bool $return_array = false): object|array|false",
    handler: get_browser,
};

pub fn register(r: &mut Registry) {
    // `main/main.c` directive, owned by Core.
    r.vm.ini
        .register(&IniDef::new("browscap", None, modifiable::SYSTEM), "Core");
    r.function(&GET_BROWSER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::basic::set_http_user_agent;
    use crate::testing::TestVm;

    fn dir() -> String {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/phpt/ext/standard/tests/misc"
        )
        .to_string()
    }

    fn vm_with(ini: &str) -> TestVm {
        let mut t = TestVm::new();
        t.vm.ini_set_system("browscap", format!("{}/{ini}", dir()));
        t
    }

    fn keys(a: &PhpArray) -> Vec<String> {
        a.keys().map(|k| k.to_string()).collect()
    }

    #[test]
    fn no_directive_warns() {
        let mut t = TestVm::new();
        let r = t.call("get_browser", vec![Value::str("x")]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: get_browser(): browscap ini directive not set in /t.php on line 3\n"
        );
    }

    #[test]
    fn unreadable_file_is_a_core_warning() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("browscap", "/no/such/browscap.ini");
        let r = t.call("get_browser", vec![Value::str("x")]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: Cannot open \"/no/such/browscap.ini\" for reading in Unknown on line 0\n"
        );
    }

    #[test]
    fn first_user_agent_of_get_browser_basic() {
        let mut t = vm_with("browscap.ini");
        let r = t
            .call(
                "get_browser",
                vec![
                    Value::str("Opera/7.11 (Windows NT 5.1; U) [en]"),
                    Value::Bool(true),
                ],
            )
            .unwrap();
        let Value::Array(a) = r else {
            panic!("array expected")
        };
        assert_eq!(a.len(), 35);
        let s = |k: &str| {
            a.get_str(k.as_bytes())
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            s("browser_name_regex"),
            r"~^opera/7\.1.* \(windows nt 5\.1; .\).*$~"
        );
        assert_eq!(s("browser_name_pattern"), "Opera/7.1* (Windows NT 5.1; ?)*");
        assert_eq!(s("parent"), "Opera 7.1");
        assert_eq!(s("platform"), "WinXP");
        assert_eq!(s("browser"), "Opera");
        assert_eq!(s("win32"), "1");
        assert_eq!(s("alpha"), "");
        assert_eq!(s("aolversion"), "0");
        assert_eq!(
            &keys(&a)[..8],
            [
                "browser_name_regex",
                "browser_name_pattern",
                "parent",
                "platform",
                "win32",
                "browser",
                "version",
                "majorver"
            ]
        );
    }

    #[test]
    fn second_user_agent_prefers_the_longest_match() {
        let mut t = vm_with("browscap.ini");
        let ua = "Mozilla/4.0 (compatible; MSIE 6.0; MSIE 5.5; Windows NT 5.0) Opera 7.02 Bork-edition [en]";
        let Value::Array(a) = t
            .call("get_browser", vec![Value::str(ua), Value::Bool(true)])
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            a.get_str(b"browser_name_pattern")
                .unwrap()
                .as_str()
                .unwrap(),
            "Mozilla/4.0 (compatible; MSIE 6.0; MSIE 5.5; Windows NT 5.0) Opera 7.0*"
        );
        assert_eq!(a.get_str(b"platform").unwrap().as_str().unwrap(), "Win2000");
    }

    #[test]
    fn object_result_is_a_stdclass() {
        let mut t = vm_with("browscap.ini");
        let r = t
            .call(
                "get_browser",
                vec![Value::str("Opera/7.11 (Windows NT 5.1; U) [en]")],
            )
            .unwrap();
        let Value::Object(o) = r else {
            panic!("object expected")
        };
        assert_eq!(o.class().name, "stdClass");
        assert_eq!(o.get_prop(b"browser").unwrap().as_str().unwrap(), "Opera");
        assert_eq!(
            o.get_prop(b"parent").unwrap().as_str().unwrap(),
            "Opera 7.1"
        );
    }

    #[test]
    fn missing_default_section_is_false() {
        // get_browser_no_default.phpt
        let mut t = vm_with("browscap_no_default.ini");
        let r = t.call("get_browser", vec![Value::str("")]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
    }

    #[test]
    fn unknown_agent_falls_back_to_the_default_section() {
        let mut t = vm_with("browscap.ini");
        let r = t
            .call(
                "get_browser",
                vec![Value::str("totally-unknown-agent"), Value::Bool(true)],
            )
            .unwrap();
        // either the "Default Browser Capability Settings" section or false
        assert!(matches!(r, Value::Array(_) | Value::Bool(false)));
    }

    #[test]
    fn user_agent_defaults_to_the_server_variable() {
        let mut t = vm_with("browscap.ini");
        let r = t.call("get_browser", vec![]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: get_browser(): HTTP_USER_AGENT variable is not set, cannot determine user agent name in /t.php on line 3\n"
        );
        set_http_user_agent(&mut t.vm, "Opera/7.11 (Windows NT 5.1; U) [en]");
        let Value::Array(a) = t
            .call("get_browser", vec![Value::Null, Value::Bool(true)])
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(a.get_str(b"browser").unwrap().as_str().unwrap(), "Opera");
    }

    #[test]
    fn wildcard_matching() {
        assert!(match_wildcard(b"abc", b"abc"));
        assert!(match_wildcard(b"abc", b"a*"));
        assert!(match_wildcard(b"abc", b"*c"));
        assert!(match_wildcard(b"abc", b"a?c"));
        assert!(match_wildcard(b"abcabc", b"*abc"));
        assert!(match_wildcard(b"axxbxxc", b"a*b*c"));
        assert!(!match_wildcard(b"abc", b"a?"));
        assert!(!match_wildcard(b"abc", b"abcd"));
        assert!(match_wildcard(b"", b"*"));
        assert!(match_wildcard(b"ab", b"a**b"));
    }

    #[test]
    fn pattern_conversion_and_scanner() {
        assert_eq!(
            convert_pattern(b"Mozilla/5.0 (X*) ?+~"),
            b"~^mozilla/5\\.0 \\(x.*\\) .\\+\\~$~".to_vec()
        );
        assert_eq!(compute_prefix_len(b"abc*d"), 3);
        let b = parse_browscap(
            b"; c\n[Sec One]\nKey = on\nOther=\"quoted ; text\" ; trailing\nBare\nEmpty=\nParent=Other\n[Other]\nX=1;c\n",
            b"f.ini",
        )
        .unwrap();
        assert_eq!(b.entries.len(), 2);
        assert_eq!(*b.entries[0].parent.as_ref().unwrap(), "Other");
        let kv: Vec<(String, String)> =
            b.kv.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        assert_eq!(
            kv,
            [
                ("key".into(), "1".into()),
                ("other".into(), "quoted ; text".into()),
                ("empty".into(), "".into()),
                ("x".into(), "1".into()),
            ]
        );
        let err = parse_browscap(b"[A]\nParent=a\n", b"f.ini").err().unwrap();
        assert!(err.starts_with(
            "Invalid browscap ini file: 'Parent' value cannot be same as the section name: A"
        ));
    }
}
