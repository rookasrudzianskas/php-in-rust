//! `libmbfl/filters/mbfilter_htmlent.c` and `html_entities.c`: the
//! (deprecated) "HTML-ENTITIES" pseudo-encoding. Non-ASCII codepoints are
//! written as named entities (HTML 4) or decimal numeric entities; decoding
//! accepts named, decimal and hexadecimal entities.
//!
//! The legacy filters are kept because `mbfl_strcut` walks HTML-ENTITIES
//! strings with them; the decoder's entity buffer lives in
//! [`ConvertFilter::opaque`].

use crate::mbstring::mbfl::convert::ConvertBuf;
use crate::mbstring::mbfl::encoding::{ENCTYPE_GL_UNSAFE, Encoding, NoEncoding};
use crate::mbstring::mbfl::filter::{ConvertFilter, FilterOutput, Vtbl, ck, common_ctor};

pub static MBFL_ENCODING_HTML_ENT: Encoding = Encoding {
    no: NoEncoding::HtmlEnt,
    name: "HTML-ENTITIES",
    mime_name: Some("HTML-ENTITIES"),
    aliases: &["HTML", "html"],
    mblen_table: None,
    flag: ENCTYPE_GL_UNSAFE,
    input_filter: Some(&VTBL_HTML_WCHAR),
    output_filter: Some(&VTBL_WCHAR_HTML),
    to_wchar: mb_htmlent_to_wchar,
    from_wchar: mb_wchar_to_htmlent,
    check: None,
    cut: None,
};

pub static VTBL_WCHAR_HTML: Vtbl = Vtbl {
    from: NoEncoding::Wchar,
    to: NoEncoding::HtmlEnt,
    ctor: common_ctor,
    function: mbfl_filt_conv_html_enc,
    flush: mbfl_filt_conv_html_enc_flush,
};

pub static VTBL_HTML_WCHAR: Vtbl = Vtbl {
    from: NoEncoding::HtmlEnt,
    to: NoEncoding::Wchar,
    ctor: mbfl_filt_conv_html_dec_ctor,
    function: mbfl_filt_conv_html_dec,
    flush: mbfl_filt_conv_html_dec_flush,
};

/// `mbfl_html_entity_list` (`html_entities.c`): entity names and their
/// codepoints, in php-src order (the first match wins in both directions).
pub static MBFL_HTML_ENTITY_LIST: &[(&str, u32)] = &[
    ("quot", 34),
    ("amp", 38),
    ("lt", 60),
    ("gt", 62),
    ("nbsp", 160),
    ("iexcl", 161),
    ("cent", 162),
    ("pound", 163),
    ("curren", 164),
    ("yen", 165),
    ("brvbar", 166),
    ("sect", 167),
    ("uml", 168),
    ("copy", 169),
    ("ordf", 170),
    ("laquo", 171),
    ("not", 172),
    ("shy", 173),
    ("reg", 174),
    ("macr", 175),
    ("deg", 176),
    ("plusmn", 177),
    ("sup2", 178),
    ("sup3", 179),
    ("acute", 180),
    ("micro", 181),
    ("para", 182),
    ("middot", 183),
    ("cedil", 184),
    ("sup1", 185),
    ("ordm", 186),
    ("raquo", 187),
    ("frac14", 188),
    ("frac12", 189),
    ("frac34", 190),
    ("iquest", 191),
    ("Agrave", 192),
    ("Aacute", 193),
    ("Acirc", 194),
    ("Atilde", 195),
    ("Auml", 196),
    ("Aring", 197),
    ("AElig", 198),
    ("Ccedil", 199),
    ("Egrave", 200),
    ("Eacute", 201),
    ("Ecirc", 202),
    ("Euml", 203),
    ("Igrave", 204),
    ("Iacute", 205),
    ("Icirc", 206),
    ("Iuml", 207),
    ("ETH", 208),
    ("Ntilde", 209),
    ("Ograve", 210),
    ("Oacute", 211),
    ("Ocirc", 212),
    ("Otilde", 213),
    ("Ouml", 214),
    ("times", 215),
    ("Oslash", 216),
    ("Ugrave", 217),
    ("Uacute", 218),
    ("Ucirc", 219),
    ("Uuml", 220),
    ("Yacute", 221),
    ("THORN", 222),
    ("szlig", 223),
    ("agrave", 224),
    ("aacute", 225),
    ("acirc", 226),
    ("atilde", 227),
    ("auml", 228),
    ("aring", 229),
    ("aelig", 230),
    ("ccedil", 231),
    ("egrave", 232),
    ("eacute", 233),
    ("ecirc", 234),
    ("euml", 235),
    ("igrave", 236),
    ("iacute", 237),
    ("icirc", 238),
    ("iuml", 239),
    ("eth", 240),
    ("ntilde", 241),
    ("ograve", 242),
    ("oacute", 243),
    ("ocirc", 244),
    ("otilde", 245),
    ("ouml", 246),
    ("divide", 247),
    ("oslash", 248),
    ("ugrave", 249),
    ("uacute", 250),
    ("ucirc", 251),
    ("uuml", 252),
    ("yacute", 253),
    ("thorn", 254),
    ("yuml", 255),
    ("OElig", 338),
    ("oelig", 339),
    ("Scaron", 352),
    ("scaron", 353),
    ("Yuml", 376),
    ("fnof", 402),
    ("circ", 710),
    ("tilde", 732),
    ("Alpha", 913),
    ("Beta", 914),
    ("Gamma", 915),
    ("Delta", 916),
    ("Epsilon", 917),
    ("Zeta", 918),
    ("Eta", 919),
    ("Theta", 920),
    ("Iota", 921),
    ("Kappa", 922),
    ("Lambda", 923),
    ("Mu", 924),
    ("Nu", 925),
    ("Xi", 926),
    ("Omicron", 927),
    ("Pi", 928),
    ("Rho", 929),
    ("Sigma", 931),
    ("Tau", 932),
    ("Upsilon", 933),
    ("Phi", 934),
    ("Chi", 935),
    ("Psi", 936),
    ("Omega", 937),
    ("alpha", 945),
    ("beta", 946),
    ("gamma", 947),
    ("delta", 948),
    ("epsilon", 949),
    ("zeta", 950),
    ("eta", 951),
    ("theta", 952),
    ("iota", 953),
    ("kappa", 954),
    ("lambda", 955),
    ("mu", 956),
    ("nu", 957),
    ("xi", 958),
    ("omicron", 959),
    ("pi", 960),
    ("rho", 961),
    ("sigmaf", 962),
    ("sigma", 963),
    ("tau", 964),
    ("upsilon", 965),
    ("phi", 966),
    ("chi", 967),
    ("psi", 968),
    ("omega", 969),
    ("thetasym", 977),
    ("upsih", 978),
    ("piv", 982),
    ("ensp", 8194),
    ("emsp", 8195),
    ("thinsp", 8201),
    ("zwnj", 8204),
    ("zwj", 8205),
    ("lrm", 8206),
    ("rlm", 8207),
    ("ndash", 8211),
    ("mdash", 8212),
    ("lsquo", 8216),
    ("rsquo", 8217),
    ("sbquo", 8218),
    ("ldquo", 8220),
    ("rdquo", 8221),
    ("bdquo", 8222),
    ("dagger", 8224),
    ("Dagger", 8225),
    ("bull", 8226),
    ("hellip", 8230),
    ("permil", 8240),
    ("prime", 8242),
    ("Prime", 8243),
    ("lsaquo", 8249),
    ("rsaquo", 8250),
    ("oline", 8254),
    ("frasl", 8260),
    ("euro", 8364),
    ("weierp", 8472),
    ("image", 8465),
    ("real", 8476),
    ("trade", 8482),
    ("alefsym", 8501),
    ("larr", 8592),
    ("uarr", 8593),
    ("rarr", 8594),
    ("darr", 8595),
    ("harr", 8596),
    ("crarr", 8629),
    ("lArr", 8656),
    ("uArr", 8657),
    ("rArr", 8658),
    ("dArr", 8659),
    ("hArr", 8660),
    ("forall", 8704),
    ("part", 8706),
    ("exist", 8707),
    ("empty", 8709),
    ("nabla", 8711),
    ("isin", 8712),
    ("notin", 8713),
    ("ni", 8715),
    ("prod", 8719),
    ("sum", 8721),
    ("minus", 8722),
    ("lowast", 8727),
    ("radic", 8730),
    ("prop", 8733),
    ("infin", 8734),
    ("ang", 8736),
    ("and", 8743),
    ("or", 8744),
    ("cap", 8745),
    ("cup", 8746),
    ("int", 8747),
    ("there4", 8756),
    ("sim", 8764),
    ("cong", 8773),
    ("asymp", 8776),
    ("ne", 8800),
    ("equiv", 8801),
    ("le", 8804),
    ("ge", 8805),
    ("sub", 8834),
    ("sup", 8835),
    ("nsub", 8836),
    ("sube", 8838),
    ("supe", 8839),
    ("oplus", 8853),
    ("otimes", 8855),
    ("perp", 8869),
    ("sdot", 8901),
    ("lceil", 8968),
    ("rceil", 8969),
    ("lfloor", 8970),
    ("rfloor", 8971),
    ("lang", 9001),
    ("rang", 9002),
    ("loz", 9674),
    ("spades", 9824),
    ("clubs", 9827),
    ("hearts", 9829),
    ("diams", 9830),
];

/// `htmlentitifieds`: 1 for bytes which are written as entities (0x80-0xFF);
/// `&`, `<` and `>` are marked 2 but still written literally.
static HTMLENTITIFIEDS: [u8; 256] = {
    let mut t = [0u8; 256];
    t[b'&' as usize] = 2;
    t[b'<' as usize] = 2;
    t[b'>' as usize] = 2;
    let mut i = 0x80;
    while i < 256 {
        t[i] = 1;
        i += 1;
    }
    t
};

/// any => HTML.
pub fn mbfl_filt_conv_html_enc(c: i32, _f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    if (0..256).contains(&c) && HTMLENTITIFIEDS[c as usize] != 1 {
        ck!(out.output(c));
    } else {
        ck!(out.output(b'&' as i32));
        if let Some(&(name, _)) = MBFL_HTML_ENTITY_LIST
            .iter()
            .find(|&&(_, code)| c as u32 == code)
        {
            for &b in name.as_bytes() {
                ck!(out.output(b as i32));
            }
        } else {
            ck!(out.output(b'#' as i32));
            // `c` is printed as unsigned (BAD_INPUT becomes 4294967295)
            let mut digits = [0u8; 10];
            let mut p = digits.len();
            let mut uc = c as u32;
            loop {
                p -= 1;
                digits[p] = b'0' + (uc % 10) as u8;
                uc /= 10;
                if uc == 0 {
                    break;
                }
            }
            for &d in &digits[p..] {
                ck!(out.output(d as i32));
            }
        }
        ck!(out.output(b';' as i32));
    }
    0
}

pub fn mbfl_filt_conv_html_enc_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    f.status = 0;
    f.opaque.clear();
    out.flush();
    0
}

/// `html_enc_buffer_size`.
const HTML_ENC_BUFFER_SIZE: i32 = 16;

/// `strchr(html_entity_chars, c)`: note that `strchr` also finds the NUL
/// terminator, so a NUL byte counts as an entity character.
fn is_html_entity_chars(c: i32) -> bool {
    let b = c as u8;
    b == 0 || b == b'#' || b.is_ascii_alphanumeric()
}

/// HTML => any. `status` is the number of bytes held in the entity buffer
/// (`opaque`, `html_enc_buffer_size + 1` bytes; it starts with `&`).
pub fn mbfl_filt_conv_html_dec_ctor(f: &mut ConvertFilter) {
    f.status = 0;
    f.opaque = vec![0; HTML_ENC_BUFFER_SIZE as usize + 1];
}

/// Outputs and empties the entity buffer: `while (filter->status--)`, which
/// returns early (leaving `status` as is) when the output fails.
fn html_dec_flush_buffer(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut pos = 0usize;
    loop {
        let st = f.status;
        f.status -= 1;
        if st == 0 {
            break;
        }
        let e = out.output(f.opaque[pos]);
        pos += 1;
        if e != 0 {
            return e;
        }
    }
    f.status = 0;
    0
}

pub fn mbfl_filt_conv_html_dec(c: i32, f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let mut ent: u32 = 0;

    if f.status == 0 {
        if c == b'&' as i32 {
            f.status = 1;
            f.opaque[0] = b'&' as i32;
        } else {
            ck!(out.output(c));
        }
    } else if c == b';' as i32 {
        if f.opaque[1] == b'#' as i32 {
            if f.status > 2 && (f.opaque[2] == b'x' as i32 || f.opaque[2] == b'X' as i32) {
                if f.status > 3 {
                    // numeric entity; overflow wraps around as in php-src
                    for pos in 3..f.status as usize {
                        let v = f.opaque[pos];
                        let v = if (b'0' as i32..=b'9' as i32).contains(&v) {
                            v - b'0' as i32
                        } else if (b'A' as i32..=b'F' as i32).contains(&v) {
                            v - b'A' as i32 + 10
                        } else if (b'a' as i32..=b'f' as i32).contains(&v) {
                            v - b'a' as i32 + 10
                        } else {
                            ent = u32::MAX;
                            break;
                        };
                        ent = ent.wrapping_mul(16).wrapping_add(v as u32);
                    }
                } else {
                    ent = u32::MAX;
                }
            } else {
                // numeric entity
                if f.status > 2 {
                    for pos in 2..f.status as usize {
                        if ent > 0x1999_9999 {
                            ent = u32::MAX;
                            break;
                        }
                        let v = f.opaque[pos];
                        if (b'0' as i32..=b'9' as i32).contains(&v) {
                            ent = ent.wrapping_mul(10).wrapping_add((v - b'0' as i32) as u32);
                        } else {
                            ent = u32::MAX;
                            break;
                        }
                    }
                } else {
                    ent = u32::MAX;
                }
            }
            if ent < 0x11_0000 {
                ck!(out.output(ent as i32));
            } else {
                for pos in 0..f.status.max(0) as usize {
                    ck!(out.output(f.opaque[pos]));
                }
                ck!(out.output(c));
            }
            f.status = 0;
        } else {
            // named entity: `strcmp(buffer + 1, name)` on the NUL-terminated buffer
            f.opaque[f.status as usize] = 0;
            let name_end = (1..f.opaque.len())
                .find(|&i| f.opaque[i] == 0)
                .unwrap_or(f.opaque.len());
            let name = &f.opaque[1..name_end];
            if let Some(&(_, code)) = MBFL_HTML_ENTITY_LIST.iter().find(|&&(n, _)| {
                n.len() == name.len() && n.bytes().zip(name).all(|(a, &b)| a as i32 == b)
            }) {
                ent = code;
            }
            if ent != 0 {
                // decoded
                ck!(out.output(ent as i32));
                f.status = 0;
            } else {
                // failure
                f.opaque[f.status as usize] = b';' as i32;
                f.status += 1;
                f.opaque[f.status as usize] = 0;

                // flush fragments
                let e = html_dec_flush_buffer(f, out);
                if e != 0 {
                    return e;
                }
            }
        }
    } else {
        // add character
        f.opaque[f.status as usize] = c & 0xFF;
        f.status += 1;
        // add character and check
        if !is_html_entity_chars(c)
            || f.status + 1 == HTML_ENC_BUFFER_SIZE
            || (c == b'#' as i32 && f.status > 2)
        {
            // illegal character or end of buffer
            if c == b'&' as i32 {
                f.status -= 1;
            }
            f.opaque[f.status as usize] = 0;

            let e = html_dec_flush_buffer(f, out);
            if e != 0 {
                return e;
            }

            if c == b'&' as i32 {
                f.opaque[f.status as usize] = b'&' as i32;
                f.status += 1;
            }
        }
    }
    0
}

pub fn mbfl_filt_conv_html_dec_flush(f: &mut ConvertFilter, out: &mut dyn FilterOutput) -> i32 {
    let status = f.status;
    f.status = 0;
    let mut err = 0;

    // flush fragments
    for pos in 0..status.max(0) as usize {
        let e = out.output(f.opaque[pos]);
        if e != 0 {
            err = e;
        }
    }

    out.flush();
    err
}

fn is_html_entity_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'#'
}

/// Parses the digits of a numeric entity (`&#...;` without `&#` and `;`);
/// `None` is php-src's `goto bad_entity`. Overflow wraps around as in
/// php-src before the range check.
fn parse_numeric_entity(digits: &[u8]) -> Option<u32> {
    let mut value: u32 = 0;
    if let Some(hex) = digits
        .strip_prefix(b"x")
        .or_else(|| digits.strip_prefix(b"X"))
    {
        // Hexadecimal
        if hex.is_empty() {
            return None;
        }
        for &digit in hex {
            let v = match digit {
                b'0'..=b'9' => digit - b'0',
                b'A'..=b'F' => digit - b'A' + 10,
                b'a'..=b'f' => digit - b'a' + 10,
                _ => return None,
            };
            value = value.wrapping_mul(16).wrapping_add(v as u32);
        }
    } else {
        // Decimal
        if digits.is_empty() {
            return None;
        }
        for &digit in digits {
            if !digit.is_ascii_digit() {
                return None;
            }
            value = value.wrapping_mul(10).wrapping_add((digit - b'0') as u32);
        }
    }
    if value > 0x10_FFFF { None } else { Some(value) }
}

pub fn mb_htmlent_to_wchar(input: &mut &[u8], out: &mut [u32], _state: &mut u32) -> usize {
    let s = *input;
    let e = s.len();
    let mut p = 0usize;
    let mut o = 0usize;
    let limit = out.len();

    while p < e && o < limit {
        let c = s[p];
        p += 1;

        if c == b'&' {
            // Find terminating ; for HTML entity
            let mut terminator = p;
            while terminator < e && is_html_entity_char(s[terminator]) {
                terminator += 1;
            }
            if terminator < e && s[terminator] == b';' {
                if s[p] == b'#' && e - p >= 2 {
                    // Numeric entity
                    if let Some(value) = parse_numeric_entity(&s[p + 1..terminator]) {
                        out[o] = value;
                        o += 1;
                        p = terminator + 1;
                        continue;
                    }
                } else if terminator > p {
                    // Named entity
                    let name = &s[p..terminator];
                    if let Some(&(_, code)) = MBFL_HTML_ENTITY_LIST
                        .iter()
                        .find(|&&(n, _)| n.as_bytes() == name)
                    {
                        out[o] = code;
                        o += 1;
                        p = terminator + 1;
                        continue;
                    }
                }
            }
            // Either we didn't find ;, or the name of the entity was not recognized
            out[o] = b'&' as u32;
            o += 1;
            while p < terminator && o < limit {
                out[o] = s[p] as u32;
                o += 1;
                p += 1;
            }
            if terminator < e && s[terminator] == b';' && o < limit {
                out[o] = s[p] as u32;
                o += 1;
                p += 1;
            }
        } else {
            out[o] = c as u32;
            o += 1;
        }
    }

    *input = &s[p..];
    o
}

pub fn mb_wchar_to_htmlent(input: &[u32], buf: &mut ConvertBuf, _end: bool) {
    for &w in input {
        if w < 256 && HTMLENTITIFIEDS[w as usize] != 1 {
            // Fast path for most ASCII characters
            buf.add(w as u8);
            continue;
        }

        buf.add(b'&');

        // See if there is a matching named entity
        if let Some(&(name, _)) = MBFL_HTML_ENTITY_LIST.iter().find(|&&(_, code)| w == code) {
            buf.append(name.as_bytes());
            buf.add(b';');
            continue;
        }

        // There is no matching named entity; emit a numeric entity instead
        buf.add(b'#');
        let mut digits = [0u8; 12];
        let mut p = digits.len();
        let mut w = w;
        if w == 0 {
            p -= 1;
            digits[p] = b'0';
        }
        while w != 0 {
            p -= 1;
            digits[p] = b'0' + (w % 10) as u8;
            w /= 10;
        }
        buf.append(&digits[p..]);
        buf.add(b';');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbstring::filters::utf8::MBFL_ENCODING_UTF8;
    use crate::mbstring::mbfl::convert::{decode_all, encode_all, mb_fast_convert};
    use crate::mbstring::mbfl::encoding::{BAD_INPUT, ILLEGAL_MODE_CHAR, NoEncoding, enc};
    use crate::mbstring::mbfl::filter::convert_filter_new;
    use crate::mbstring::mbfl::strcut::mbfl_strcut;

    const HTML: &Encoding = &MBFL_ENCODING_HTML_ENT;
    const UTF8: &Encoding = &MBFL_ENCODING_UTF8;

    struct Collect(Vec<i32>);
    impl FilterOutput for Collect {
        fn output(&mut self, c: i32) -> i32 {
            self.0.push(c);
            0
        }
        fn flush(&mut self) -> i32 {
            0
        }
    }

    fn conv(s: &[u8], from: &'static Encoding, to: &'static Encoding) -> Vec<u8> {
        mb_fast_convert(s, from, to, b'?' as u32, ILLEGAL_MODE_CHAR).0
    }

    /// Decodes with the legacy filter (the one `mbfl_strcut` uses).
    fn legacy_decode(s: &[u8]) -> Vec<u32> {
        let mut f = convert_filter_new(HTML, enc(NoEncoding::Wchar)).unwrap();
        let mut out = Collect(Vec::new());
        for &c in s {
            f.feed(c as i32, &mut out);
        }
        f.flush(&mut out);
        out.0.into_iter().map(|c| c as u32).collect()
    }

    /// Encodes with the legacy filter.
    fn legacy_encode(cps: &[u32]) -> Vec<u8> {
        let mut f = convert_filter_new(enc(NoEncoding::Wchar), HTML).unwrap();
        let mut out = Collect(Vec::new());
        for &c in cps {
            f.feed(c as i32, &mut out);
        }
        f.flush(&mut out);
        out.0.into_iter().map(|c| c as u8).collect()
    }

    /// htmlent_encoding.phpt
    #[test]
    fn htmlent_encoding() {
        let to_entities = |raw: &str, ent: &str| {
            assert_eq!(conv(raw.as_bytes(), UTF8, HTML), ent.as_bytes(), "{raw:?}");
            let cps: Vec<u32> = raw.chars().map(|c| c as u32).collect();
            assert_eq!(legacy_encode(&cps), ent.as_bytes(), "{raw:?}");
        };
        let from_entities = |raw: &str, ent: &str| {
            assert_eq!(conv(ent.as_bytes(), HTML, UTF8), raw.as_bytes(), "{ent:?}");
            let cps: Vec<u32> = raw.chars().map(|c| c as u32).collect();
            assert_eq!(legacy_decode(ent.as_bytes()), cps, "{ent:?}");
        };
        let both = |raw: &str, ent: &str| {
            to_entities(raw, ent);
            from_entities(raw, ent);
        };

        both("", "");
        both("abc", "abc");
        both("&<>", "&<>");

        from_entities("\"", "&quot;");
        from_entities("\u{EC}", "&igrave;");

        from_entities("あ", "&#x3042;");
        both("あ", "&#12354;");
        both("abcあxyz", "abc&#12354;xyz");

        from_entities("&#x;", "&#x;");
        from_entities("&#;", "&#;");
        from_entities("&#", "&#");
        from_entities("&", "&");

        from_entities("\0", "&#00000;");

        both(&"あ".repeat(100), &"&#12354;".repeat(100));

        from_entities("&;", "&;");
        from_entities("&f;", "&f;");

        from_entities("&A", "&&#65;");
        from_entities("&A", "&&#x41;");
    }

    /// htmlent.phpt: HTML-ENTITIES input decoded, then re-encoded for output.
    #[test]
    fn htmlent_round_trip() {
        let input = b"&#38;&#64;&#65;&#66;&#128;&#129;&#130;&auml;&ouml;&uuml;&euro;&lang;&rang;";
        let decoded = decode_all(input, HTML);
        assert_eq!(decoded, legacy_decode(input));
        let buf = encode_all(&decoded, HTML, b'?' as u32, ILLEGAL_MODE_CHAR);
        assert_eq!(
            buf.out,
            b"&@AB&#128;&#129;&#130;&auml;&ouml;&uuml;&euro;&lang;&rang;"
        );
    }

    #[test]
    fn entity_edge_cases() {
        // Out of range and overflowing numeric entities
        assert_eq!(
            conv(b"&#1114112;&#x110000;", HTML, UTF8),
            b"&#1114112;&#x110000;"
        );
        assert_eq!(conv(b"&#x100000041;", HTML, UTF8), b"A");
        assert_eq!(legacy_decode(b"&#x100000041;"), [0x41]);
        assert_eq!(
            legacy_decode(b"&#4294967361;"),
            "&#4294967361;"
                .chars()
                .map(|c| c as u32)
                .collect::<Vec<_>>()
        );
        // Unknown names and unterminated entities stay literal
        assert_eq!(
            conv(b"&nosuch; &amp &lt;x", HTML, UTF8),
            b"&nosuch; &amp <x"
        );
        let s = b"&nosuch; &amp &lt;x";
        assert_eq!(legacy_decode(s), decode_all(s, HTML));
        // The legacy decoder flushes its buffer once it holds 15 bytes
        let long = b"&abcdefghijklmnopq;";
        assert_eq!(
            legacy_decode(long),
            long.iter().map(|&b| b as u32).collect::<Vec<_>>()
        );
        // ...and, comparing the names with strcmp, stops at a NUL byte
        assert_eq!(legacy_decode(b"&lt\0x;"), [0x3C]);

        // The legacy encoder prints BAD_INPUT as an unsigned number
        assert_eq!(
            legacy_encode(&[0x26, 0xE4, 0x3042, 0x20AC, BAD_INPUT]),
            b"&&auml;&#12354;&euro;&#4294967295;"
        );
        assert_eq!(
            encode_all(
                &[0x26, 0xE4, 0x3042, 0x20AC],
                HTML,
                b'?' as u32,
                ILLEGAL_MODE_CHAR
            )
            .out,
            b"&&auml;&#12354;&euro;"
        );
    }

    #[test]
    fn small_output_buffer() {
        let input = b"x&amp;&#12354;&bogus;y";
        let mut all = Vec::new();
        let mut buf = [0u32; 2];
        let mut state = 0;
        let mut inp = &input[..];
        while !inp.is_empty() {
            let n = mb_htmlent_to_wchar(&mut inp, &mut buf, &mut state);
            all.extend_from_slice(&buf[..n]);
        }
        assert_eq!(all, decode_all(input, HTML));
        assert_eq!(
            all,
            [
                0x78, 0x26, 0x3042, 0x26, 0x62, 0x6F, 0x67, 0x75, 0x73, 0x3B, 0x79
            ]
        );
    }

    #[test]
    fn strcut() {
        let cut = |s: &[u8], from: usize, len: usize| mbfl_strcut(s, HTML, from, len).unwrap();
        let s = b"abc&#12354;xyz";
        assert_eq!(cut(s, 0, s.len()), s);
        assert_eq!(cut(s, 0, 3), b"abc");
        assert_eq!(cut(s, 3, 100), b"&#12354;xyz");
    }
}
