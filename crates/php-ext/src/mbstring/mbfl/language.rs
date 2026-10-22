//! `libmbfl/mbfl/mbfl_language.c` and `libmbfl/nls/*.c`.

use super::encoding::NoEncoding;

/// `enum mbfl_no_language` (only the values php-src defines languages for
/// matter; the numbering is php-src's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoLanguage {
    Neutral,
    Uni,
    German,
    English,
    Japanese,
    Korean,
    SimplifiedChinese,
    TraditionalChinese,
    Russian,
    Ukrainian,
    Armenian,
    Turkish,
}

/// `mbfl_language`.
#[derive(Debug)]
pub struct Language {
    pub no: NoLanguage,
    pub name: &'static str,
    pub short_name: &'static str,
    pub aliases: &'static [&'static str],
    pub mail_charset: NoEncoding,
    pub mail_header_encoding: NoEncoding,
    pub mail_body_encoding: NoEncoding,
}

const fn lang(
    no: NoLanguage,
    name: &'static str,
    short_name: &'static str,
    aliases: &'static [&'static str],
    mail_charset: NoEncoding,
    mail_header_encoding: NoEncoding,
    mail_body_encoding: NoEncoding,
) -> Language {
    Language {
        no,
        name,
        short_name,
        aliases,
        mail_charset,
        mail_header_encoding,
        mail_body_encoding,
    }
}

use NoEncoding as E;
use NoLanguage as L;

/// `mbfl_language_ptr_table`, in php-src order.
pub static LANGUAGES: [Language; 12] = [
    lang(L::Uni, "uni", "uni", &["universal"], E::Utf8, E::Base64, E::Base64),
    lang(L::Japanese, "Japanese", "ja", &[], E::Iso2022jp, E::Base64, E::SevenBit),
    lang(L::Korean, "Korean", "ko", &[], E::Iso2022kr, E::Base64, E::SevenBit),
    lang(L::SimplifiedChinese, "Simplified Chinese", "zh-cn", &[], E::Hz, E::Base64, E::SevenBit),
    lang(L::TraditionalChinese, "Traditional Chinese", "zh-tw", &[], E::Big5, E::Base64, E::EightBit),
    lang(L::English, "English", "en", &[], E::Iso8859_1, E::Qprint, E::EightBit),
    lang(L::German, "German", "de", &["Deutsch"], E::Iso8859_15, E::Qprint, E::EightBit),
    lang(L::Russian, "Russian", "ru", &[], E::Koi8r, E::Qprint, E::EightBit),
    lang(L::Ukrainian, "Ukrainian", "ua", &[], E::Koi8u, E::Qprint, E::EightBit),
    lang(L::Armenian, "Armenian", "hy", &[], E::Armscii8, E::Qprint, E::EightBit),
    lang(L::Turkish, "Turkish", "tr", &[], E::Iso8859_9, E::Qprint, E::EightBit),
    lang(L::Neutral, "neutral", "neutral", &[], E::Utf8, E::Base64, E::Base64),
];

/// `mbfl_name2language`: names, then short names, then aliases
/// (`strcasecmp`, so the name ends at the first NUL byte).
pub fn name2language(name: &[u8]) -> Option<&'static Language> {
    let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
    LANGUAGES
        .iter()
        .find(|l| l.name.as_bytes().eq_ignore_ascii_case(name))
        .or_else(|| {
            LANGUAGES
                .iter()
                .find(|l| l.short_name.as_bytes().eq_ignore_ascii_case(name))
        })
        .or_else(|| {
            LANGUAGES.iter().find(|l| {
                l.aliases
                    .iter()
                    .any(|a| a.as_bytes().eq_ignore_ascii_case(name))
            })
        })
}

/// `mbfl_no2language`.
pub fn no2language(no: NoLanguage) -> &'static Language {
    LANGUAGES
        .iter()
        .find(|l| l.no == no)
        .expect("every NoLanguage has a descriptor")
}
