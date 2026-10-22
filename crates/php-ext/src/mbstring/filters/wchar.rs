//! `libmbfl/mbfl/mbfilter_wchar.c`: the internal "wchar" pseudo-encoding
//! (codepoints), the pivot of the legacy filters.

use crate::mbstring::filters::pass::{pass_to_wchar, wchar_to_pass};
use crate::mbstring::mbfl::encoding::{ENCTYPE_WCS4, Encoding, NoEncoding};

pub static MBFL_ENCODING_WCHAR: Encoding = Encoding {
    no: NoEncoding::Wchar,
    name: "wchar",
    mime_name: None,
    aliases: &[],
    mblen_table: None,
    flag: ENCTYPE_WCS4,
    input_filter: None,
    output_filter: None,
    to_wchar: pass_to_wchar,
    from_wchar: wchar_to_pass,
    check: None,
    cut: None,
};
