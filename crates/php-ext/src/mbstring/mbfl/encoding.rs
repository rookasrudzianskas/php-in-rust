//! `libmbfl/mbfl/mbfl_encoding.{c,h}` and `mbfl_consts.h`: the encoding
//! descriptor, the registry in php-src order and name lookup.

use super::convert::ConvertBuf;
use super::filter::Vtbl;

/// `MBFL_ENCTYPE_SBCS`: one byte per character.
pub const ENCTYPE_SBCS: u32 = 0x0000_0001;
/// `MBFL_ENCTYPE_WCS2`: two bytes per character.
pub const ENCTYPE_WCS2: u32 = 0x0000_0002;
/// `MBFL_ENCTYPE_WCS4`: four bytes per character.
pub const ENCTYPE_WCS4: u32 = 0x0000_0004;
/// `MBFL_ENCTYPE_GL_UNSAFE`: bytes < 0x80 may be part of multi-byte characters.
pub const ENCTYPE_GL_UNSAFE: u32 = 0x0000_4000;

pub const WCSPLANE_UCS2MAX: u32 = 0x0001_0000;
pub const WCSPLANE_UTF32MAX: u32 = 0x0011_0000;
pub const WCSPLANE_SUPMIN: u32 = 0x0001_0000;
pub const WCSPLANE_SUPMAX: u32 = 0x0020_0000;

/// `MBFL_BAD_INPUT`: the error marker produced by decoders.
pub const BAD_INPUT: u32 = u32::MAX;

/// `MBFL_OUTPUTFILTER_ILLEGAL_MODE_*`.
pub const ILLEGAL_MODE_NONE: u32 = 0;
pub const ILLEGAL_MODE_CHAR: u32 = 1;
pub const ILLEGAL_MODE_LONG: u32 = 2;
pub const ILLEGAL_MODE_ENTITY: u32 = 3;
/// For internal use only; uses the invalid UTF-8 byte 0xFF as error marker.
pub const ILLEGAL_MODE_BADUTF8: u32 = 4;

/// `MBSTRING_MIN_WCHAR_BUFSIZE`.
pub const MIN_WCHAR_BUFSIZE: usize = 5;

/// `enum mbfl_no_encoding`, in php-src order (range checks depend on it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i32)]
pub enum NoEncoding {
    Invalid = -1,
    Pass,
    Wchar,
    Base64,
    Uuencode,
    HtmlEnt,
    Qprint,
    SevenBit,
    EightBit,
    CharsetMin,
    Ucs4,
    Ucs4be,
    Ucs4le,
    Ucs2,
    Ucs2be,
    Ucs2le,
    Utf32,
    Utf32be,
    Utf32le,
    Utf16,
    Utf16be,
    Utf16le,
    Utf8,
    Utf8Docomo,
    Utf8KddiA,
    Utf8KddiB,
    Utf8Sb,
    Utf7,
    Utf7imap,
    Ascii,
    EucJp,
    Eucjp2004,
    Sjis,
    EucjpWin,
    SjisDocomo,
    SjisKddi,
    SjisSb,
    SjisMac,
    Sjis2004,
    Cp932,
    Sjiswin,
    Cp51932,
    Jis,
    Iso2022jp,
    Iso2022jp2004,
    Iso2022jpKddi,
    Iso2022jpms,
    Gb18030,
    Gb18030_2022,
    Cp1252,
    Cp1254,
    Iso8859_1,
    Iso8859_2,
    Iso8859_3,
    Iso8859_4,
    Iso8859_5,
    Iso8859_6,
    Iso8859_7,
    Iso8859_8,
    Iso8859_9,
    Iso8859_10,
    Iso8859_13,
    Iso8859_14,
    Iso8859_15,
    EucCn,
    Cp936,
    EucTw,
    Big5,
    Cp950,
    EucKr,
    Iso2022kr,
    Uhc,
    Hz,
    Cp1251,
    Cp866,
    Koi8r,
    Koi8u,
    Iso8859_16,
    Armscii8,
    Cp850,
    Cp50220,
    Cp50221,
    Cp50222,
    CharsetMax,
}

/// `mb_to_wchar_fn`: decodes bytes from the front of `input` (advancing it)
/// into `out` (at most `out.len()` codepoints); returns the number written.
pub type ToWcharFn = fn(input: &mut &[u8], out: &mut [u32], state: &mut u32) -> usize;
/// `mb_from_wchar_fn`: encodes codepoints into `buf`; `end` flushes state.
pub type FromWcharFn = fn(input: &[u32], buf: &mut ConvertBuf, end: bool);
/// `mb_check_fn`: specialised validity check.
pub type CheckFn = fn(input: &[u8]) -> bool;
/// `mb_cut_fn`: `mb_strcut` for encodings with a dedicated implementation;
/// `s` is the whole string.
pub type CutFn = fn(s: &[u8], from: usize, len: usize) -> Vec<u8>;

/// `mbfl_encoding`.
pub struct Encoding {
    pub no: NoEncoding,
    pub name: &'static str,
    /// `None` is php-src's `NULL`.
    pub mime_name: Option<&'static str>,
    pub aliases: &'static [&'static str],
    pub mblen_table: Option<&'static [u8; 256]>,
    pub flag: u32,
    /// Old-style byte-at-a-time filters (only used by `mb_strcut`).
    pub input_filter: Option<&'static Vtbl>,
    pub output_filter: Option<&'static Vtbl>,
    pub to_wchar: ToWcharFn,
    pub from_wchar: FromWcharFn,
    pub check: Option<CheckFn>,
    pub cut: Option<CutFn>,
}

impl Encoding {
    /// Fixed byte width (`flag & (SBCS | WCS2 | WCS4)`): 1, 2, 4 or 0.
    pub fn char_len(&self) -> usize {
        (self.flag & (ENCTYPE_SBCS | ENCTYPE_WCS2 | ENCTYPE_WCS4)) as usize
    }

    pub fn is(&self, no: NoEncoding) -> bool {
        self.no == no
    }

    /// `php_mb_is_no_encoding_utf8`: UTF-8 or one of its mobile variants.
    pub fn is_utf8_family(&self) -> bool {
        self.no >= NoEncoding::Utf8 && self.no <= NoEncoding::Utf8Sb
    }
}

impl PartialEq for Encoding {
    fn eq(&self, other: &Encoding) -> bool {
        self.no == other.no
    }
}

impl std::fmt::Debug for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Encoding({})", self.name)
    }
}

pub use super::encodings::ENCODING_LIST;

/// `mbfl_name2encoding_ex`: primary names first, then MIME names, then
/// aliases, all ASCII case-insensitive.
pub fn name2encoding(name: &[u8]) -> Option<&'static Encoding> {
    // php-src's perfect hash only covers primary names of 2..=23 bytes.
    if (2..=23).contains(&name.len())
        && let Some(e) = ENCODING_LIST
            .iter()
            .find(|e| e.name.as_bytes().eq_ignore_ascii_case(name))
    {
        return Some(e);
    }
    if let Some(e) = ENCODING_LIST.iter().find(|e| {
        e.mime_name
            .is_some_and(|m| m.as_bytes().eq_ignore_ascii_case(name))
    }) {
        return Some(e);
    }
    ENCODING_LIST
        .iter()
        .find(|e| {
            e.aliases
                .iter()
                .any(|a| a.as_bytes().eq_ignore_ascii_case(name))
        })
        .copied()
}

/// `mbfl_name2encoding`: the name is a C string (stops at the first NUL).
pub fn name2encoding_cstr(name: &[u8]) -> Option<&'static Encoding> {
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    name2encoding(&name[..end])
}

/// `mbfl_no2encoding`.
pub fn no2encoding(no: NoEncoding) -> Option<&'static Encoding> {
    match no {
        NoEncoding::Pass => Some(&super::super::filters::pass::MBFL_ENCODING_PASS),
        NoEncoding::Wchar => Some(&super::super::filters::wchar::MBFL_ENCODING_WCHAR),
        _ => ENCODING_LIST.iter().find(|e| e.no == no).copied(),
    }
}

/// `mbfl_no2encoding` for encodings known to exist.
pub fn enc(no: NoEncoding) -> &'static Encoding {
    no2encoding(no).expect("registered encoding")
}

/// `mbfl_no_encoding2name`.
pub fn no_encoding2name(no: NoEncoding) -> &'static str {
    no2encoding(no).map_or("", |e| e.name)
}

/// `mbfl_encoding_preferred_mime_name`.
pub fn preferred_mime_name(e: &Encoding) -> Option<&'static str> {
    e.mime_name.filter(|m| !m.is_empty())
}
